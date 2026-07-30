# Production Readiness Audit — opc-da-client v0.3.0

> **范围**：`opc-da-client/src/`（库 crate）。`opc-cli` TUI 不在本报告范围。
> **方法**：基于 `com_worker.rs` / `backend/connector.rs` / `backend/opc_da.rs` / `subscription.rs` / `opc_da/client/iterator.rs` / `com_guard.rs` 的代码级证据，所有引用都带 `<file>:<line>` + 函数名 + 片段。
> **原则**：本报告**只列证据**，不做"我相信是这样"——能 grep 到行号的才写。
>
> **结论先行**：当前实现**功能完整、API 设计干净**，但**长时间运行的鲁棒性有明显缺口**。
> 两个 P0 阻塞生产部署：(1) 订阅运行期断线无自动续订（**仅影响使用订阅的长跑场景**，短读/写不受影响）；(2) worker panic 信息被完全丢弃（P0-3 + P0-4）。
> 多 host 并发属**功能缺失**（架构选择），**已降为 P1，非生产阻塞**——单 host 部署完全可用。

---

## 总体评级矩阵

| 维度 | 判定 | 阻塞生产？ |
| --- | --- | --- |
| 1. 断线重连（连接级） | 🟡 部分落地 | **是**（订阅路径） |
| 2. 异常恢复（worker panic / drop） | 🟡 部分落地 | **是**（panic 信息丢失） |
| 3. 并发订阅（多 group 持多 subscription） | 🟡 部分落地 | 否（功能可用） |
| 4. 多连接（多 host 并发） | ❌ 缺失（架构限制） | 否（功能缺失而非鲁棒性 bug；单 host 部署完全可用，详见 §4） |
| 5. 高性能（批量 / 缓存 / 锁粒度） | 🟡 部分落地 | 否（功能可用，但延迟/吞吐有上限） |
| 6. 资源生命周期 | 🟢 落地（VARIANT 泄漏 v0.3.0 已修） | — |
| 7. 可测试性（mock 注入） | 🟢 落地 | — |
| 8. 文档与真相源一致性 | 🟢 落地（v0.3.0 已同步） | — |

> **生产就绪度评级**：**B-**。
> 功能维度满分；鲁棒性 + 多连接两个维度拖后腿，达到生产可用需修完 P0 + 大半 P1（详见 §10 优先级）。

---

## 1. 断线重连（连接失效后自动恢复）

### 1.1 已落地 ✅

**连接池按 ProgID 缓存**：

- `com_worker.rs:282` worker 线程入口 `let mut cache: HashMap<String, C::Server> = HashMap::new();`——`cache: &mut HashMap<String, C::Server>`（worker 内本地哈希表，无外部 `Arc<Mutex>`）
- `com_worker.rs:616-627` `dispatch_with_retry` 中 `cache.entry(server_name.to_string())`——按 ProgID 缓存/复用 `Server` 句柄

**失败检测（关键 RPC HRESULT 列表）**：

- `com_worker.rs:248-258` `is_connection_error()` 枚举以下 RPC 失效码：
  - `0x800706BA`（`RPC_S_SERVER_UNAVAILABLE`）
  - `0x800706BF`（`RPC_S_SERVER_CALL_FAILED`）
  - `0x800706BE`（`RPC_S_CALL_FAILED_DNE`）
  - `0x80080005`（`CO_E_SERVER_EXEC_FAILURE`）

**驱逐策略**：

- `com_worker.rs:629-637` 在 `is_connection_error()` 返回 true 时 `cache.remove(server_name)`，无条件驱逐失效句柄

**指数退避**：

- `com_worker.rs:613` `const MAX_RECONNECT_ATTEMPTS: u32 = 3;`（共 4 次尝试：1 初始 + 3 重试）
- `com_worker.rs:638-646` `let backoff_ms = 50u64 << attempt; // 50ms, 100ms, 200ms`

**测试覆盖**：

- `com_worker.rs:2206 test_stale_connection_eviction` ✅

**手动 Reconnect API**：

- `backend/opc_da.rs:62 OpcDaClient::reconnect(&self)` 把 `ComRequest::Reconnect { server }` 投给 worker
- `com_worker.rs:452` worker `match` Reconnect 分支：清缓存 + 驱逐失效代理

### 1.2 硬阻塞 ❌

**订阅（subscription）断线无自动续订**：

- `com_worker.rs:463-478` `ComRequest::Subscribe { server, group, rate, deadband, items }` 分支**不经过** `dispatch_with_retry`
- 同理 `SetSubscriptionRate`（`com_worker.rs:507-531`）与 `Unsubscribe` 也都不感知重连
- 后果：一次 RPC 失效后，`IOPCDataCallback::OnDataChange` 静默死亡，应用层必须自己写 `ShutdownRequest` 监听 + `Subscribe` 重新发起循环
- `KeepAlive` 仅作用于 group 内部（`IOPCGroupStateMgt2::SetKeepAlive`，DA 3.0），**不解决 RPC 失效后 callback 重建**

**（已修正）cache key 用 ProgID 是正确设计，非 bug**：

- `com_worker.rs:616` `cache.entry(server_name)`——单 client 内 host 固定，ProgID 唯一，作 key 完全正确
- 真正限制是「单 `OpcDaClient` 绑死单 host」的架构选择，详见 §4

---

## 2. 异常恢复（worker panic / drop 保护）

### 2.1 已落地 ✅

**COM RAII（线程局部）**：

- `com_guard.rs:34-77` `ComGuard` 在 worker 线程内 `CoInitializeEx(MTA)`，`Drop` 调 `CoUninitialize`——**即使 panic 也走 drop**（Rust 的 `Drop` unwind-safe 保证）
- `com_worker.rs:268-280` worker 入口绑定 `_guard`

**Init 阶段 panic 感知**：

- `com_worker.rs:264-267` `let (init_tx, init_rx) = std::sync::mpsc::channel();`
- `com_worker.rs:566-567` `init_rx.recv().map_err(|_| OpcError::Internal("COM worker thread panicked during init".into()))??;`——客户端可感知

**运行时 panic 感知**：

- `com_worker.rs:582-589` `send_request()` 入口检查 `JoinHandle::is_finished()` 返回 "panicked" 错误
- `com_worker.rs:594-600` 发送失败 → `"COM worker channel closed (worker stopped)"`；接收失败 → `"COM worker shut down during request"`
- 客户端可感知 worker 已死，但拿不到 panic 信息

**测试**：

- `com_worker.rs:2251 test_worker_panic_propagation` ✅

### 2.2 硬阻塞 ❌

**`JoinHandle` 从未被 `.join()`**：

- `ComWorker` 持有 `std::thread::JoinHandle`（worker 线程句柄），但 `Drop` (`com_worker.rs:1653-1657`) 仅打日志，**不调用 `take()` + `join()`** 拿 panic payload
- 后果：panic 时 root-cause 被吞，只剩一行 `tracing::error!`——生产环境线上出问题没有 panic message、没有 backtrace、没有 stack

**没有 `catch_unwind` 包裹 worker 主循环**：

- `com_worker.rs:286` `while let Some(req) = rx.blocking_recv() { ... }`——单 unwinding，**任何 panic 直接 kill worker 线程**
- `ComGuard::drop` 会跑（好事），但 `cache` / `subscriptions` HashMap 释放顺序未受控，可能留 COM 指针悬空

**in-flight 请求被静默取消（无 partial result）**：

- worker panic 后 channel 关闭，所有 `oneshot::Receiver::await` 的任务收到 `OpcError::Internal("COM worker shut down during request")`
- **客户端能感知但无 partial 数据**——browse tags 这种长任务即使已推进到 8000/10000 也全部丢失

**`ComWorker::drop` 不等待 worker 线程结束**：

- `com_worker.rs:1653-1657` 仅日志，进程退出时 worker 可能还在执行 COM 调用
- 应 `take JoinHandle` + `join().ok()`（允许限时 join）

---

## 3. 并发订阅（多 group 持多 subscription）

### 3.1 已落地 ✅

**多 group 持多 subscription（cookie 区分）**：

- `com_worker.rs:283` `let mut subscriptions: HashMap<u32, SubscriptionEntry<C>> = HashMap::new();`
- `com_worker.rs:1543-1551` 每次 `Subscribe` 通过 `group.advise_data_callback` 返回的 cookie (`u32`) 索引

**`IOPCDataCallback` sink 实现**：

- `subscription.rs:34-41` `DataCallbackSink { tag_ids: Vec<String>, tx: Mutex<mpsc::Sender<TagValue>> }`
- `subscription.rs:43-111` `IOPCDataCallback_Impl` 实现：
  - `OnDataChange`
  - `OnReadComplete`
  - `OnWriteComplete`
  - `OnCancelComplete`
- `subscription.rs:1532` `let sink_callback: IOPCDataCallback = sink.into();`（`windows::core::implement!` 宏产生 COM 包装）

**callback 跨线程送回 tokio**：

- `subscription.rs:134-158` `forward_data_change()` 用 `tx.try_send(tv)`（**非阻塞**）
- `subscription.rs:157` 注释明确 `"Non-blocking: a stalled consumer must not freeze the COM worker thread."`

**`IOPCShutdown` sink**：

- `subscription.rs:165-181 ShutdownSink`

**`IConnectionPointContainer` 包装**：

- `traits/connection_point_container.rs:9-61`
- `connection_point_container.rs:36-40` `data_callback_connection_point()` 用 `IOPCDataCallback::IID`

**`Advise` / `Unadvise` 真正调用**：

- `connector.rs:810-815` `ConnectedGroup::advise_data_callback` → `cp.Advise(sink)`
- `connector.rs:817-824` `ConnectedGroup::unadvise_data_callback` → `cp.Unadvise(cookie)`
- `connector.rs:583-603` server-level `IOPCShutdown` 的 Advise/Unadvise

### 3.2 不足 🟡（功能可用，但生产需要加固）

**callback 线程的 Send/Sync 正确性未显式文档化**：

- `#[implement(IOPCDataCallback)]` 宏生成 vtable 函数，签名是 `&self`
- 文档说 "OPC server invokes callbacks on a non-COM-worker thread"（reverse-DCOM callback 是 RPC runtime 起的线程）
- 但 `DataCallbackSink` 字段 `Mutex<mpsc::Sender<TagValue>>` 跨线程访问的同步原语正确，**Send/Sync 应该由编译器推断保证**
- **建议**在 `DataCallbackSink` 加 `// SAFETY: ...` 注释或在文档中明确线程模型（不是 bug，但生产事故排查时这是盲区）

**`SubscriptionEntry.sink` 字段仅保引用计数**：

- `com_worker.rs:228-229` `sink: windows::core::IUnknown` 标了 `#[allow(dead_code)]`，**仅靠字段存在维持 COM 引用计数**（`windows::core::IUnknown` AddRef/Release 自动管）
- 若 `SubscriptionHandle` 被客户端 drop 但 cookie 没显式 `Unsubscribe`，订阅**一直活着**直到 `ComWorker::drop` 触发 channel 关闭
- 不是 bug，是"显式 unsubscribe 是契约"——但应在 `subscription.rs` 顶部注释里**写明**

**`subscription.rs:134` 静默吞错**：

- `let Ok(tx) = tx.lock() else { return; };`——Mutex 中毒时静默返回
- 调试时无可见信号
- 建议改 `let Ok(tx) = tx.lock() else { tracing::warn!(...); return; };`

**callback 通道容量有界**：

- `com_worker.rs:1526` `let (tx, rx) = mpsc::channel(256);` 给 `DataCallbackSink`
- 上限 256 是合理的——但客户端 `SubscriptionHandle.rx` 应按需 `recv` 消费，否则 sink 端 `try_send` 失败被 `let _ = ...;` 吞掉，**数据丢失无信号**
- 建议加计数器或 `tracing::warn!(dropped=N)` 在 sink 端

---

## 4. 多连接（多 host 并发）

### 4.1 已落地 ✅

- 单 worker 内 `HashMap<String, C::Server>` 缓存多 server
- 测试：`com_worker.rs:2169 test_connection_cache_reuse` 验证复用 ✅

### 4.2 架构限制 🟡（功能缺失，非鲁棒性 bug）

**根因：`ComConnector.host` 构造时绑死 → 单 `OpcDaClient` 绑死单 host**：

- `connector.rs:321-323` `pub struct ComConnector { host: String }`——host 是实例字段，构造时固定
- `connector.rs:339-342` `impl Default` → `Self::new("localhost")`——`OpcDaClient::default()` 永远只连本机
- `connector.rs:377-378` `connect()` 内部 `connect_server(server_name, &self.host)`——**连接目标 host 取决于 connector 实例，不取自请求**
- `backend/opc_da.rs:15-17` `OpcDaClient { worker: ComWorker<C> }`，`ComWorker` 持 `Arc<C>` 单 connector（`opc_da.rs:36`）——**一个 client = 一个固定 host**

**cache key 用 ProgID 是正确设计，不是 bug**：

- `com_worker.rs:282` `cache: HashMap<String, C::Server>`，key = ProgID
- 单 client 内 host 固定 → ProgID 唯一 → 用 ProgID 作 key 完全正确
- 旧版报告「缓存 key 缺 host 维度」的表述是误导，已修正：真正缺的不是 key 维度，而是「单 client 无法承载多 host」

**`enumerate_servers` 与 `connect` 的 host 语义不一致**（本次复核补充）：

- `connector.rs:348` `enumerate_servers(&self, host: &str)`——host 是**方法参数**，可枚举任意 host
- `connector.rs:377` `connect(&self, server_name)`——用 `self.host` **固定值**
- 后果：`list_servers("192.168.199.155")` 能列出远程服务器的 ProgID，但后续 `read`/`write`/`browse` 连的是 `self.host`（本机）——**能看见却连不上**（除非 client 的 connector 恰好绑了那个 host）。这是 API 层面的语义割裂，建议在文档中显式标注，或让 `connect` 也接受 host 参数（见 P1-7）

**同 ProgID 跨 host 行为**：

- `com_worker.rs:616` `cache.entry(server_name)`——同一 client 内同 ProgID 命中旧 entry（设计正确）
- 要连 Matrikon(本机) + Kepware(本机) 不同 ProgID ✅（同 host）
- 要连 Matrikon(本机) + Matrikon(远程 192.168.199.155) ❌（需多 client 实例）
- 要连「同一 ProgID 在两台机器同时连」 ❌（需多 client 实例）

**结论**：这是**功能缺失，不是生产部署的硬阻塞**。单 host 部署（最常见场景）完全可用。要支持多 host 必须多 `OpcDaClient` 实例（每个独立 worker 线程 + 独立连接池），Rust async runtime 下无生命周期管理 API，应用层自行拼装。若业务确有多 host 并发需求，见 §10 P1-7（原 P0-2，已降级）。

---

## 5. 高性能（缓存 / 锁粒度 / 批量）

### 5.1 已落地 ✅

**批量读写单次 COM 调用**：

- `com_worker.rs:1126-1268 handle_write_values` 一次 `add_items` + 一次 `group.write(&handles, &variants)`
- `com_worker.rs:658-803 handle_read` 一次 `add_items` + 一次 `group.read`
- 同理 `handle_read_max_age`（`com_worker.rs:805-901`）+ `handle_write_vqt`

**tags_sink 超时收割（partial result）**：

- `com_worker.rs:60` `tags_sink: Arc<std::sync::Mutex<Vec<String>>>`
- `com_worker.rs:58` `progress: Arc<AtomicUsize>`
- `com_worker.rs:1299-1302` flat 路径每发现一个 tag 立即 push 到 sink
- `com_worker.rs:1315-1330` hierarchical/OPC_FLAT 路径同步 push
- `com_worker.rs:1426-1429` recursive 路径同步 push
- 锁失败时 `if let Ok(...)` 跳过——**不会 panic**

**`Vec::with_capacity` 预分配（部分覆盖）**：

- `connector.rs:553` `Vec::with_capacity(ids_slice.len())`
- `connector.rs:772` `Vec::with_capacity(n)`
- `com_worker.rs:728-729` `server_handles: Vec::new(); valid_indices: Vec::new();` ❌ **未预分配**

**通道容量**：

- `com_worker.rs:263` `mpsc::channel(32)` 请求队列
- `com_worker.rs:1526` `mpsc::channel(256)` DataCallbackSink
- `com_worker.rs:1612` `mpsc::channel(8)` ShutdownSink

**Iterator 跨 FFI 缓存**：

- `opc_da/client/iterator.rs:7` `const MAX_CACHE_SIZE: usize = 16;`
- `opc_da/client/iterator.rs:8` `const STRING_CACHE_SIZE: usize = 256;`
- `iterator.rs:42-69 GuidIterator::next` 走 `Box<[GUID; 16]>` 缓存
- `iterator.rs:95+ StringIterator::next` 走 `Box<[PWSTR; 256]>` 缓存
- 避免每次 Next 跨 COM FFI

### 5.2 不足 🟡

**worker 处理请求串行**：

- `com_worker.rs:286` `while let Some(req) = rx.blocking_recv()`——单线程循环
- `mpsc::channel(32)` 容量 → 高并发请求被 `.send().await` 阻塞
- **没有优先队列、超时调度、请求批处理**（browse 与短读在同队列 FIFO）

**每次 op 都 `add_group`/`remove_group`**：

- `com_worker.rs:673-683` 每次 `handle_read` 都 `add_group("opc-da-client-read", ...)` → `remove_group`——**没有 persistent group 池**
- 每个 read/write = `AddGroup → AddItems → Read/Write → RemoveGroup`（4 次 COM 调用，不是单次 `SyncIO::Read`）
- 高频读场景（1000 QPS）下成为延迟主因

**tags_sink 锁粒度粗**：

- `std::sync::Mutex<Vec<String>>`——每条 tag 都争锁一次
- 10k 标签显著序列化

**没有用 `parking_lot`**：

- `rg parking_lot` 0 命中
- `std::sync::Mutex` 在 Windows 下够用但不是最优（parking_lot 在 Windows 用 `SRWLock` + futex）

**BSTR / 字符串分配**：

- `com_worker.rs:685-688` 每个 tag_id 一次 `encode_utf16().chain(once(0)).collect()`（new `Vec<u16>`）
- add_items 后整个 vec 立即 drop；批量 N tag = N 次堆分配
- 可改成一次性 buffer + offsets（slice split）

**Vec 预分配覆盖不足**：

- `com_worker.rs:865-866` `let mut handles: Vec<ItemHandle> = Vec::new();` 未预分配 `tag_ids.len()`
- `com_worker.rs:1188-1195` `write_results` 走 `iter().map().collect()`（OK，隐式预分配）

**每个 op 一个 `oneshot::channel`**：

- `com_worker.rs:591` 每个 op 一个 `oneshot::channel()`
- 无批量收割接口（high-frequency read pipeline 无法共享 channel）

---

## 6. 资源生命周期 🟢（v0.3.0 P0 已修）

- `com_utils.rs::clear_variant_array` / `clear_item_states` 在 `RemotePointer::drop`（只 `CoTaskMemFree`）之前 `VariantClear` 每个 VARIANT 的 BSTR/SafeArray
- 7 处生产路径泄漏点已修复：`handle_read` / `handle_write` / `handle_write_vqt` / `handle_write_values` / `get_item_properties` / `read_max_age`
- CHANGELOG 0.3.0 P0 条目 ✅

---

## 7. 可测试性 🟢

- `OpcProvider` 是稳定 async trait（18 方法）
- `OpcDaClient<C: ServerConnector>` 泛型注入 mock
- `test-support` feature 通过 `mockall::automock` 生成 `MockOpcProvider`
- CI 可在任意 OS 跑（无需真 OPC 服务器）

---

## 8. 文档与真相源一致性 🟢（v0.3.0 P1 已同步）

- `spec.md` OpcProvider 表 4→**18 方法**（含完整签名 + COM 接口映射）
- `CHANGELOG.md` 0.3.0 完整条目（Added / Fixed / Changed-breaking）
- 根 `README.md` features 区反映实际能力（remote DCOM / subscription 已实现）
- `opc-da-client/README.md` 版本 0.3 + full feature list
- `Cargo.toml` `version = "0.3.0"`

---

## 9. 与生产级目标库的差距（横向对照）

| 能力 | opc-da-client | OpenOPC | python-opcua |
| --- | --- | --- | --- |
| 自动重连（连接级） | ✅（订阅无） | ⚠ 部分 | ✅ |
| Subscription 自动续订 | ❌ | ⚠ 部分 | ✅ |
| Worker panic 可观察 | ❌ JoinHandle 未 join | N/A | ✅ (asyncio) |
| 多 host 并发 | ❌ 同 ProgID 跨 host | ⚠ 单 host | ✅ |
| 批量读 / 写 | ✅ | ✅ | ✅ |
| 持久 group 池 | ❌ | ⚠ | ✅ |
| 反压（backpressure） | ❌ 固定 mpsc(32) | ⚠ | ✅ |

---

## 10. 改进建议（按优先级）

### 10.0 判断速览：必须修 vs 值得修 vs 可选

> 复核（2026-07-30）后的执行判断。**BUG 全部属实**（逐行核实，证据见 §13），但"是否必须修"取决于部署形态。

| 类别 | 项 | 判断依据 |
| --- | --- | --- |
| **必须修** | **P0-3 + P0-4**（panic 可观察性） | 真实、独立、工作量小（~1d）、风险低；不修则线上 worker 崩溃无 root cause（无 panic message / backtrace）。**任何生产部署都该先做这两个** |
| **必须修（按场景）** | **P0-1**（订阅运行期续订） | 仅当**使用订阅 + 长时间运行**时是硬阻塞；纯短读/写场景不受影响。修复复杂（3-5d）且有 reverse-DCOM 外部依赖，按业务节奏排期 |
| **值得修** | **P1-4**（try_send warn）、**P1-3**（文档单 host） | trivial（各 ~0.25d），顺手做，显著改善可观测性 / 可理解性 |
| **值得修（按负载）** | **P1-1**（group 池） | 高频读（>数百 QPS）才成为延迟瓶颈；低频场景可缓 |
| **可选 / 按需** | **P1-7**（多 host，原 P0-2） | 功能缺失非 bug，单 host 部署不需要；仅多 host 并发业务才做 |
| **可选 / 按需** | **P1-2**（lock-free sink）、**P1-5**（Drop 主动 unsubscribe） | 10k+ 标签 / 严格要求订阅不泄漏才值得；P1-5 有 async drop 陷阱 |
| **低优先级** | **P1-6**（parking_lot）、**P2-*** | 收益有限或属性能优化，非阻塞 |

**一句话结论**：先做 **P0-3 + P0-4**（1 天，最高性价比）；若用订阅长跑则加 **P0-1**；其余按业务负载和场景取舍。多 host（P1-7）已从"阻塞"降级为"按需功能"。

### P0 — 阻塞生产部署（必须修）

| ID | 任务 | 涉及文件 | 估时 |
| --- | --- | --- | --- |
| **P0-3** | `ComWorker::drop` 里 `take JoinHandle + join()` 拿 panic payload，附 `tracing::error!` 完整 message + backtrace。注意：drop 中 `join` 会阻塞调用方（worker 可能正卡在长 COM 调用），需 detach 或限时 join | `com_worker.rs:1653-1657` | 0.25 d |
| **P0-4** | `catch_unwind` 包裹 worker 主循环每次迭代，捕获 panic payload 记 `tracing::error!`。**语义修正**：panic 后 `cache`/`subscriptions` 内 COM 指针状态一致性无法保证，建议「记录 payload + 关闭 channel」（保留当前 client 可感知语义，仅补回被吞的 message），**而非**「worker 不死继续跑」——后者有状态损坏风险。`catch_unwind` 需 `AssertUnwindSafe`（COM 指针非 `UnwindSafe`） | `com_worker.rs:286` | 0.5 d |
| **P0-1** | 订阅**运行期**断线自动续订：callback 存活监测（KeepAlive 超时 / 最近 `OnDataChange` 时间戳）+ 失效后 `unadvise` → 重新 `advise` → 重新 `add_items`。**仅「建立订阅接入 `dispatch_with_retry`」不够**——那只解决建立期重连，不解决运行中 RPC 断线后 `IOPCDataCallback` 静默死亡。且 reverse-DCOM callback 重建依赖客户端允许入站 DCOM（见 CLAUDE.md 已知坑），完全自动化有外部配置前提 | `com_worker.rs:463-531, 1561-1576` + 新增监测逻辑 | 3-5 d |

P0-3 + P0-4 同源（panic 可观察性）——**优先级最高，一起修**（真实、独立、工作量小、风险低）。
P0-1 仅对**使用订阅的长时间运行**场景是硬阻塞；短读/写场景不受影响，可按业务节奏排期。
原 P0-2（多 host）已降级为 **P1-7**（功能缺失，非阻塞）。

### P1 — 显著提升生产可用性

| ID | 任务 | 涉及文件 | 估时 |
| --- | --- | --- | --- |
| **P1-1** | persistent group 池：按 `(host, prog_id, group_name, purpose)` 缓存 `ComGroup`，read/write 复用；加 TTL 驱逐 | `com_worker.rs:673-683` | 2 d |
| **P1-2** | tags_sink lock-free 化：用 `crossbeam::channel` 或 batch flush（每 N tag 或 50ms flush 一次） | `com_worker.rs:60` | 1 d |
| **P1-3** | `OpcProvider` 文档 + `OpcDaClient` 文档**显式说明** "每个 client 实例绑定一个 host；多 host 需要多个 client 实例" | `provider.rs` + `backend/opc_da.rs` | 0.25 d |
| **P1-4** | DataCallbackSink `try_send` 失败时 `tracing::warn!(dropped=N)` 而非静默吞 | `subscription.rs:134-158` | 0.25 d |
| **P1-5** | SubscriptionHandle `Drop` 时**主动** unsubscribe（防订阅静默存活）。**注意 Rust 陷阱**：`Drop::drop` 不能 `.await`，无法等待 worker 完成 unsubscribe；需向 worker channel 非阻塞投递 `ComRequest::Unsubscribe`（`try_send`）或 `tokio::spawn` | `subscription.rs` + `com_worker.rs` | 1 d |
| **P1-6** | 把 `parking_lot` 引入工作区（`std::sync::Mutex` → `parking_lot::Mutex`）。收益有限（`std::sync::Mutex` 在 Windows 用 SRWLock 已够用），优先级最低 | `Cargo.toml` + 多文件 | 0.5 d |
| **P1-7** | 多 host 支持（**原 P0-2，降级**）：`ServerConnector::connect` 加 `host` 参数 → `ComConnector` 去 host 化（host 移入请求）→ cache key 改 `(host, prog_id)` → 所有 mock/调用点同步。**功能缺失非 bug**，单 host 部署不需要；仅当业务确有多 host 并发需求时实施 | `backend/connector.rs:321-342, 377-391` + `com_worker.rs:282,616` + trait 签名 | 2-3 d |

### P2 — 性能优化空间（非阻塞）

| ID | 任务 | 涉及文件 | 估时 |
| --- | --- | --- | --- |
| **P2-1** | worker 并行化：多 worker 线程（按 host 或 ProgID hash 分发）；或读/写分离两条 worker 线程 | `com_worker.rs:286` | 5 d（含测试） |
| **P2-2** | BSTR 字符串分配优化：一次性 buffer + slice split | `com_worker.rs:685-688` | 0.5 d |
| **P2-3** | Vec 预分配补全：所有 `Vec::new()` + 已知长度场景改 `with_capacity(n)` | 多文件 | 0.5 d |
| **P2-4** | 加 worker 优先队列（browse / 订阅 > 短读 > 批量写） | `com_worker.rs:286` | 1 d |
| **P2-5** | batch 收割接口：高频读场景共享 channel | `com_worker.rs:591` | 1 d |

### P3 — 工程化

| ID | 任务 |
| --- | --- |
| **P3-1** | 加 fuzz test：随机 ComRequest 序列触发 panic 路径 |
| **P3-2** | 加 benchmark：`criterion` harness 对 read/write/browse/subscribe 的 P50/P99 延迟 |
| **P3-3** | 加 `cargo-mutants` 突变测试覆盖 `dispatch_with_retry` |
| **P3-4** | 真机 7×24 soak test：Matrikon + Kepware + 远程 192.168.199.155 同时跑，72h 无泄漏（用 `RUST_LOG=trace` 抓 VARIANT/BSTR 计数） |
| **P3-5** | CI 加 Windows Server 2022 跑 e2e feature |

---

## 11. 推荐路线图

```
                  ┌─────────────────────────────────────────┐
                  │ Phase 1: 消除生产硬阻塞                 │  ~1 d 起
                  │   P0-3 + P0-4 (panic 可观察, 先做)      │
                  │   P0-1 (订阅运行期续订, 按需)           │
                  └────────────────────┬────────────────────┘
                                       ▼
                  ┌─────────────────────────────────────────┐
                  │ Phase 2: 鲁棒性 + 易用性               │  ~5 d
                  │   P1-1 (group 池)                       │
                  │   P1-2 (lock-free tags_sink)            │
                  │   P1-3 / P1-4 / P1-5 / P1-6 / P1-7     │
                  └────────────────────┬────────────────────┘
                                       ▼
                  ┌─────────────────────────────────────────┐
                  │ Phase 3: 性能优化                       │  ~8 d
                  │   P2-1 (worker 并行化 — 风险最大)       │
                  │   P2-2 / P2-3 / P2-4 / P2-5            │
                  └────────────────────┬────────────────────┘
                                       ▼
                  ┌─────────────────────────────────────────┐
                  │ Phase 4: 验证                           │  ~持续
                  │   P3-1 / P3-2 / P3-3 / P3-4 / P3-5     │
                  └─────────────────────────────────────────┘
```

**总估时**：Phase 1（仅 P0-3 + P0-4）约 **1 个工作日**即可消除最关键的 panic 可观察性硬阻塞；含 P0-1 约 **4-6 个工作日**。Phase 1 + Phase 2 累计约 **8-10 个工作日**达 A 级生产就绪。Phase 3 视业务规模决定是否投资。

---

## 12. 测试矩阵建议

| 测试类型 | 现状 | 建议 |
| --- | --- | --- |
| 单元测试（mockall） | ✅ 47 个 | 维持 |
| Doc tests | ✅ 10 个 | 维持 |
| E2E（feature = e2e） | ✅ 19 本地 + 4 远程 | 维持 |
| 集成测试（真机多 server） | ❌ | 加：Matrikon + Kepware 同时连 |
| 故障注入（kill server） | ⚠ `test_stale_connection_eviction` | 扩展：网络断、server 崩溃、订阅中 server 重启 |
| 长时间跑（24h+） | ❌ | 加 soak test（监控 BSTR/SafeArray 计数） |
| 性能基准（criterion） | ❌ | 加 read/write/browse/subscribe 延迟分布 |
| 突变测试 | ❌ | 加 `cargo-mutants` |

---

## 13. 附录：所有引用的代码位置索引

```
com_worker.rs:25-30         ListServers 枚举定义（host 字段）
com_worker.rs:58-60         tags_sink Arc<Mutex<Vec<String>>>
com_worker.rs:228-229       SubscriptionEntry.sink IUnknown
com_worker.rs:248-258       is_connection_error() 列表
com_worker.rs:263           mpsc::channel(32) 请求队列
com_worker.rs:264-267       init_tx/init_rx
com_worker.rs:268-280       worker 入口 ComGuard 绑定
com_worker.rs:282           cache HashMap<String, C::Server>
com_worker.rs:283           subscriptions HashMap<u32, SubscriptionEntry<C>>
com_worker.rs:286           worker 主循环 while let Some(req) = rx.blocking_recv()
com_worker.rs:288-308       ListServers 分支（host 上下文丢失）
com_worker.rs:452           ComRequest::Reconnect 分支
com_worker.rs:463-478       ComRequest::Subscribe 分支（不在 dispatch_with_retry）
com_worker.rs:507-531       ComRequest::SetSubscriptionRate/SetKeepAlive 分支
com_worker.rs:566-567       init_rx.recv() panic 感知
com_worker.rs:582-600       send_request panic/channel 错误处理
com_worker.rs:613           MAX_RECONNECT_ATTEMPTS = 3
com_worker.rs:616-627       dispatch_with_retry cache.entry(server_name)
com_worker.rs:621-626       Vacant 分支 e.insert(connector.connect(...))
com_worker.rs:629-637       cache.remove(server_name) 驱逐
com_worker.rs:638-646       let backoff_ms = 50u64 << attempt
com_worker.rs:658-803       handle_read
com_worker.rs:673-683       add_group + remove_group 每次 read
com_worker.rs:685-688       tag_id encode_utf16 字符串分配
com_worker.rs:728-729       server_handles / valid_indices Vec::new()（未预分配）
com_worker.rs:805-901       handle_read_max_age
com_worker.rs:865-866       handles: Vec::new()（未预分配）
com_worker.rs:1126-1268     handle_write_values
com_worker.rs:1188-1195     write_results iter().map().collect()
com_worker.rs:1299-1302     flat browse 路径 push 到 tags_sink
com_worker.rs:1315-1330     hierarchical / OPC_FLAT browse 路径 push
com_worker.rs:1426-1429     recursive browse 路径 push
com_worker.rs:1526          DataCallbackSink mpsc::channel(256)
com_worker.rs:1543-1551     Subscribe cookie 索引
com_worker.rs:1612          ShutdownSink mpsc::channel(8)
com_worker.rs:1653-1657     ComWorker::drop（不 join）
com_worker.rs:2169          test_connection_cache_reuse
com_worker.rs:2206          test_stale_connection_eviction
com_worker.rs:2251          test_worker_panic_propagation

com_guard.rs:34-77          ComGuard CoInitializeEx(MTA) RAII

backend/opc_da.rs:15-17     OpcDaClient 单例只持一个 ComWorker<C>
backend/opc_da.rs:62        OpcDaClient::reconnect() 入口

backend/connector.rs:377-391 connect() 用 self.host（ComConnector 绑定 host）
backend/connector.rs:553     Vec::with_capacity(ids_slice.len())
backend/connector.rs:583-603 server-level IOPCShutdown Advise/Unadvise
backend/connector.rs:772     Vec::with_capacity(n)
backend/connector.rs:810-815 ConnectedGroup::advise_data_callback → cp.Advise(sink)
backend/connector.rs:817-824 ConnectedGroup::unadvise_data_callback → cp.Unadvise

subscription.rs:34-41        DataCallbackSink 字段定义
subscription.rs:43-111       IOPCDataCallback_Impl（OnDataChange / OnReadComplete / …）
subscription.rs:134-158      forward_data_change() tx.try_send（注释：non-blocking）
subscription.rs:165-181      ShutdownSink
subscription.rs:1532         let sink_callback: IOPCDataCallback = sink.into();

opc_da/client/iterator.rs:7  const MAX_CACHE_SIZE: usize = 16
opc_da/client/iterator.rs:8  const STRING_CACHE_SIZE: usize = 256
opc_da/client/iterator.rs:42-69  GuidIterator::next 缓存
opc_da/client/iterator.rs:95+    StringIterator::next 缓存

opc_da/client/traits/connection_point_container.rs:9-61  IConnectionPointContainer 包装
opc_da/client/traits/connection_point_container.rs:36-40 data_callback_connection_point()
```

---

## 14. 评审签字

| 项 | 状态 |
| --- | --- |
| 代码扫描范围 | `opc-da-client/src/` 全量 + `tests/e2e.rs` + `examples/remote_list.rs` |
| 评审方法 | grep + 函数级 read，证据全部带 `<file>:<line>` |
| 评审日期 | 2026-07-30 |
| 评审版本 | v0.3.0（commit f7a2705 + ba90d74） |
| 建议下次评审 | Phase 1（P0）完成后 |