# opc-da-server 规模化技术方案（可落地实现计划）

> 日期：2026-08-03　分支：`feat/opc-da-server`
> 状态：规划定稿，待实现　真相源：本文件 + `opc-da-server/architecture.md` + git log
> 上游设计：`docs/superpowers/specs/2026-08-02-opc-da-server-design.md`（接口/对象设计）
> 运行手册：`docs/superpowers/specs/LOOP_RUNBOOK_opc-da-server.md`（里程碑 0-4 进度）

---

## 0. 文档定位

本文档是 opc-da-server 从"接口正确的 MVP"演进到"支撑工业级并发与规模"的**实现计划**。每个阶段（P0–P4）给出：目标、设计、数据结构、改造点（精确到 `file:fn`）、验证标准、工作量、风险。实现时按 §8 规程逐阶段推进，每阶段独立 commit + 验证 + 勾选。

**不改动 `opc-da-client`**（RUNBOOK §0 硬约束：复用其 `bindings`/`com_utils`/`typedefs`，发现 client 真 bug 才修）。

---

## 1. 目标与量化指标

### 1.1 功能与规模目标

| 目标 | 指标 | 验证方式 |
|---|---|---|
| hierarchical 命名空间 | 10w+ item 分布在分支树下 | browse 导航 + 全枚举（P2 压测）|
| 并发 client | **100+** OPC client 同时连接订阅/读 | P4 达标压测 |
| 总订阅 item | **< 100w**（每 client 部分订阅，非满载）| P4 达标压测 |
| 推送吞吐 | deadband 后实际推送 << 协议上限 | P4 推送压测 |

### 1.2 协议层上限（背景，非目标）

基于 OPC DA（COM/MSRPC）协议分析（本机 LRPC、x86_64、8-16 核）：

| 指标 | 本机 LRPC | 远程 DCOM（1Gbps）|
|---|---|---|
| 单 client 订阅吞吐 | 50w–200w item/s | 1w–10w item/s |
| 并发 client 上限 | 100–500 | 几十–几百 |
| 总订阅 item 上限 | 几百万 | 百万级 |

**结论**：本目标（100+ client / <100w 订阅）处于协议**舒适区**，不在极限区。瓶颈在实现架构（线程模型）与工程质量，不在协议。

### 1.3 非目标

- 千万级订阅 / 1000+ client 远程（超 OPC DA 单 server 舒适区，应考虑 OPC UA）
- 集群/水平扩展（OPC DA COM 单对象，DCOM 无原生负载均衡）
- Windows Service 包装（属阶段 3 运维，另案）

---

## 2. 现状瓶颈（基于当前代码）

| 瓶颈 | 位置 | 影响 |
|---|---|---|
| **per-group 线程** | `publisher.rs:41-49` `spawn()` 每 GroupObj 起 1 个 `thread::spawn`；`group.rs:194` `GroupObj::new` 调用 | 1w 组 = 1w OS 线程，栈内存 + 调度灾难。**头号问题** |
| **无 deadband 过滤** | `publisher.rs:122-128` `push_data_change` 无差别全推；`GroupInner.percent_deadband`（`group.rs:97`）存了没用 | 100w 全推接近协议上限，无变化惰性 |
| **flat 命名空间** | `data_source.rs:30` `NamespaceTree{leaves}` 无树；`server.rs` `QueryOrganization=OPC_NS_FLAT`，`BrowseOPCItemIDs(BRANCH)` 返空 | 10w item 分支需求无法满足 |
| **锁内 String clone** | `group.rs:117-124` `snapshot_for_publish` 锁内 clone 全量 `String` item_id | 推送路径分配压力 |
| **无压测手段** | — | 无法量化极限、无法回归 |

已具备的并发基础（不动）：MTA free-threaded（`bin/opc-da-server.rs:90` `CoInitializeSecurity`）、`DataSource: Send+Sync`（`data_source.rs:61`）、`SyncIO::Read` 短持锁、`locked()` poison 不 panic。

---

## 3. 目标架构

```
                       ┌──────────────────────────────────────────┐
  100+ clients ─LRPC──▶│  opc-da-server (单进程, MTA)               │
                       │                                          │
                       │  COM RPC 线程池 (OS 管理, 并发处理 client 请求)
                       │    ├─ SyncIO::Read/Write ─▶ DataSource    │
                       │    ├─ AddGroup ─────────▶ 注册到 Scheduler│
                       │    ├─ RemoveGroup ──────▶ 注销 job        │
                       │    └─ Advise/Unadvise ──▶ data_cp sink 表 │
                       │                                          │
                       │  ★ 统一推送调度器 (Scheduler, P0)         │
                       │     时间轮(1ms tick) → 到期组入 job 队列   │
                       │     固定 worker 线程池(N=核数)             │
                       │       ├─ 读 DataSource                   │
                       │       ├─ ★ deadband 比较 (P1) → 过滤变化  │
                       │       └─ push OnDataChange (批量数组)     │
                       │                                          │
                       │  DataSource (P2)                         │
                       │     hierarchical 树 + 10w item HashMap 索引│
                       └──────────────────────────────────────────┘
```

**核心转变**：从"每 group 一个 OS 线程"→"全局一个调度器 + 固定线程池"。1w 组 = 1w 个轻量 `PublishJob`（每个几百字节），而非 1w 线程。

---

## 4. 分阶段实现计划

执行顺序（每阶段后跑 P4 对应压测验证）：

```
P0 统一调度 → P4(v1: 调度撑住 1w 组/100 client)
    → P1 deadband → P4(v2: 推送吞吐 + deadband 效果)
    → P2 hierarchical → P4(v3: 10w item 树 + 目标规模达标)
    → P3 按压测结果定向优化
```

---

### P0 — 统一推送调度（头号改造，先做）

**目标**：废除 per-group `thread::spawn`，改全局调度器 + 固定 worker 线程池。1w 组订阅时进程线程数 = 核数级（非 1w）。

**设计**：时间轮（按 `update_rate` 分桶）+ 固定 worker 线程池。

```rust
// 新文件：opc-da-server/src/objects/publisher/scheduler.rs（publisher.rs 拆为模块）

/// 全局推送调度器（server 启动时创建一个，GroupObj 共享）。
pub struct Scheduler {
    /// 按 update_rate 分桶（rate 离散：10/50/100/250/500/1000ms…）
    buckets: Mutex<HashMap<Duration, Bucket>>,
    /// worker 线程池（固定 N = 核数）；job 经 channel 派发
    job_tx: crossbeam_channel::Sender<PublishTask>,
    /// job 注册表（GroupObj Drop 时注销用，Arc 引用计数防竞态）
    registry: Mutex<HashMap<GroupKey, Arc<PublishJob>>>,
    shutdown: AtomicBool,
}

struct Bucket {
    jobs: Vec<Arc<PublishJob>>,      // 该 rate 下所有订阅组
    next_tick: Mutex<Instant>,       // 下次到期时间
}

/// 单个组的推送任务（数据，非线程）。
pub struct PublishJob {
    key: GroupKey,                   // 注销标识（GroupObj 的唯一 id）
    inner: Arc<Mutex<GroupInner>>,
    data_source: Arc<dyn DataSource>,
    data_cp: IConnectionPoint,       // SendCp wrapper（见 publisher.rs:34）
    rate: Duration,
    // P1 后追加：deadband 配置（从 GroupInner 读）
}

struct PublishTask { job: Arc<PublishJob> }  // 提交到 worker 的载荷
```

**调度循环**（1 个 tick 线程）：
```
loop {
    sleep_to_next_ms_boundary();
    let now = Instant::now();
    for bucket in buckets where now >= bucket.next_tick:
        bucket.next_tick += bucket.rate;
        for job in &bucket.jobs:
            job_tx.send(PublishTask{ job: job.clone() });   // 派发，不阻塞
}
```

**worker 线程**（N 个，启动时 `CoIncrementMTAUsage` 进 MTA）：
```
for task in job_rx:
    push_one(task.job);   // 复用现有 enumerate_sinks + push_data_change
```

**改造点**：
1. 新增 `objects/scheduler.rs`（Scheduler + PublishJob + tick/worker 线程）；`publisher.rs` 精简为推送数据函数（`enumerate_sinks`/`push_data_change`，已 `pub`，worker 复用）。未拆 `publisher/` 目录——职责已分（scheduler 调度+worker，publisher 数据函数）。
2. 删 `publisher.rs:41-49` `spawn()` 的 `thread::spawn`；改为 `Scheduler::register(job)`。
3. `group.rs:194` `GroupObj::new`：不再 `spawn`，改为向全局 Scheduler 注册 job。
4. `bin/opc-da-server.rs` `run_server`（87-120）：创建全局 `Scheduler`（`OnceCell` 或经 `ServerObj` 下发）。
5. `GroupObj` 实现 `Drop`：从 Scheduler 注销 job（用 `GroupKey` 查 registry；竞态见风险 R1）。
6. `GroupInner` 加 `group_key: GroupKey`（构造时分配唯一 id）。

**验证标准**：
- [ ] 单测：创建 1000 个 GroupObj，断言进程线程数 ≤ `num_cpus * 2 + 常数`（非 1000）。用 `GetProcessHandleCount` 或 `num_threads` 统计。
- [ ] 单测：job 注册/注销正确（注册 N 个，注销一半，registry 大小减半）。
- [ ] e2e：现有 13 探针全 pass（无回归）。
- [ ] P4(v1)：1w 组订阅 + 100 mock client，线程数稳定、推送正常。

**工作量**：1.5–2 天。

**风险**：
- **R1（Scheduler 生命周期竞态）**：GroupObj 在 COM RPC 线程 Drop，job 可能正被 worker 执行。缓解：`PublishJob` 用 `Arc`，worker 持 `Arc` clone；Drop 仅从 registry 移除引用，不强制终止正在执行的 job。`Arc` 引用计数保证 worker 手里的 job 存活到推送完成。
- **R2（worker 进 MTA）**：worker 调 `data_cp`/sink 是跨线程 COM，必须 MTA。worker 启动时 `CoIncrementMTAUsage`（复用 `publisher.rs:61` 模式）。

**决策点**：
- worker 池实现：自建 `std::thread` × N + `crossbeam_channel`（倾向，COM blocking 调用最直接）vs `rayon::ThreadPool`。P0 用自建，P4 压测对比后定。

---

### P1 — deadband 变化检测（高性价比保险）

**目标**：publisher 每周期只推**变化的** item，非全推。把 100w 全推降到实际变化量（典型工业稳态 1-5% → 5w 推送/s，远低于协议上限）。

**设计**：每个订阅 item cache 上次推送状态，推送前比较。

```rust
// group.rs: ItemEntry 扩展
struct ItemEntry {
    item_id: Arc<str>,              // P3.1: String → Arc<str>
    h_client: u32,
    active: bool,
    data_type: VARENUM,
    // ★ 新增：推送缓存（per-group per-item，因不同组 deadband/订阅独立）
    last_pushed: Mutex<Option<PushState>>,
}

#[derive(Clone, Copy)]
struct PushState {
    value: f64,        // 规范化（VT_I4/R8/BOOL → f64）用于 deadband 比较
    quality: u16,
    ts_serial: u64,    // FILETIME 取高低合并的序列号，避免每次比 FILETIME
}

/// 是否应推送（OPC DA deadband 语义）。
fn should_push(
    last: Option<&PushState>,
    new_value: f64,
    new_quality: u16,
    deadband_pct: f32,
    range: (f64, f64),  // item 的 EU max/min（DataSource 提供；无则 skip range 检查）
) -> bool {
    match last {
        None => true,                                    // 首次
        Some(l) if l.quality != new_quality => true,     // quality 变
        Some(l) => {
            if deadband_pct <= 0.0 { return true; }      // deadband 关闭 = 全推
            let span = (range.1 - range.0).abs();
            if span <= 0.0 { (new_value - l.value).abs() > 0.0 }
            else { (new_value - l.value).abs() >= deadband_pct as f64 * span }
        }
    }
}
```

**推送流程改造**（`worker::push_one`，原 `push_data_change`）：
```
let deadband = inner.percent_deadband;  // 锁内读一次
let frames: Vec<(h_client, item_id, value, quality, ts)> = 锁内取 active items snapshot;
// 锁外：逐 item read → should_push 比较 → 收集变化帧
let mut changed = Vec::new();
for (hc, id, ..) in frames:
    (v, q, ts) = data_source.read(&id);
    let nv = normalize(&v);  // VARIANT → f64（数值类型）
    if should_push(entry.last_pushed, nv, q, deadband, range(&id)):
        entry.last_pushed = Some(PushState{nv, q, ts_serial});
        changed.push((hc, v, q, ts));
if !changed.is_empty():
    sinks = enumerate_sinks(&data_cp);
    push_data_change(&sinks, h_group, &changed, &data_source, /*trans_id*/ 0);
// changed 空 → 不调 OnDataChange（省一次 RPC）
```

**Refresh2 例外**：client 主动刷新要全量，**绕过 deadband**（`group.rs` Refresh2 实现保持全推路径，用 `snapshot_active_for_publish`）。

**改造点**：
1. `group.rs:82-87` `ItemEntry` 加 `last_pushed: Mutex<Option<PushState>>` + `item_id: Arc<str>`。
2. `group.rs:117-137` snapshot 方法返回 `Arc<str>` 而非 `String`（配合 P3.1）。
3. `worker.rs`（原 `push_data_change`）加 deadband 循环 + `should_push`。
4. `data_source.rs` `DataSource` trait 加 `item_range(&self, id) -> Option<(f64,f64)>`（EU 范围；`SimDataSource` 返 `None`，规范数据源返实际 EU）。
5. VARIANT → f64 规范化 helper（`VT_I4`/`VT_R8`/`VT_BOOL`/`VT_I2`；非数值类型 `VT_BSTR` 等走"quality/值任意变化即推"简化语义）。

**验证标准**：
- [ ] 单测：`should_push` 各分支（首次/quality 变/值超 deadband/值未超/类型非数值）。
- [ ] 单测：`Random.Int4`（每秒变）订阅，deadband=50% 时，1s 内推送帧数 < deadband=0 时。
- [ ] 单测：deadband 关闭（0）时，行为等价 P0 全推（回归）。
- [ ] P4(v2)：deadband 开启前后，item/s 提升量级 + OnDataChange 调用次数对比。

**工作量**：1–2 天。

**风险**：
- **R3（非数值类型 deadband）**：`VT_BSTR`/`VT_ARRAY` 无 deadband 语义。简化：这些类型走"值（memcmp 或字符串比较）变化即推"，不做百分比。文档化此简化。
- **R4（deadband 范围 EU）**：需 item 的 EU max/min。`SimDataSource` 无 EU（返 `None` → deadband 退化成"任意变化即推"）。真实数据源应提供 EU。

**决策点**：
- `PushState` 存 f64 规范化值 vs 存原始 VARIANT。倾向 f64（比较快，VARIANT 在帧里单独存用于推送）。非数值类型额外存"上次字符串/原始字节"用于相等比较。

---

### P2 — hierarchical 命名空间 + 10w item DataSource（功能目标）

**目标**：browse 支持 hierarchical 分支树；DataSource 支撑 10w item O(1) 查找。

**设计**：

```rust
// data_source.rs: NamespaceTree 树化
pub enum NsNode {
    Branch { name: Arc<str>, children: Box<[NsNode]> },
    Leaf { id: Arc<str>, meta: ItemMeta, range: Option<(f64, f64)> },
}

pub struct NamespaceTree {
    root: NsNode,
    /// full path "a.b.c" → Leaf 引用索引，O(1) 查找（read/item_meta/browse）
    index: HashMap<Arc<str>, usize>,  // usize = leaf 在扁平表的序号
    leaves_flat: Box<[LeafInfo]>,     // 扁平化叶子（browse FLAT + 压测用）
}

pub struct LeafInfo { id: Arc<str>, meta: ItemMeta, range: Option<(f64, f64)> }
```

```rust
// DataSource trait 扩展
pub trait DataSource: Send + Sync {
    fn namespace(&self) -> &NamespaceTree;
    fn read(&self, item_id: &str) -> (VARIANT, u16, FILETIME);
    fn write(&self, item_id: &str, v: &VARIANT) -> HRESULT;
    fn item_meta(&self, item_id: &str) -> Option<ItemMeta>;
    fn item_range(&self, item_id: &str) -> Option<(f64, f64)>;  // ★ P1 用
    // ★ hierarchical browse
    fn browse_branch(&self, path: &[&str]) -> &[NsNode];        // 当前分支的子节点
}
```

**browse 状态机**（`server.rs` ServerObj 加 `browse_pos: Mutex<Vec<Arc<str>>>`）：

| 方法 | 当前实现 | 目标实现 |
|---|---|---|
| `QueryOrganization` | `OPC_NS_FLAT` | `OPC_NS_HIERARCHIAL` |
| `ChangeBrowsePosition(UP/DOWN/TO)` | `Ok(())` 忽略 | 更新 `browse_pos`（DOWN 压栈/UP 弹栈/TO 跳转）|
| `BrowseOPCItemIDs(BRANCH)` | 空 | 当前位置子分支 |
| `BrowseOPCItemIDs(LEAF)` | 全部 leaves | 当前位置叶子 |
| `BrowseOPCItemIDs(FLAT)` | 全部 leaves | 全部 leaves（跨分支，保留）|
| `GetItemID` | 原样返回 | 当前位置叶子 → full path |
| `BrowseAccessPaths` | 空枚举器 | 不变 |

**10w item DataSource 实现**：
- `GeneratedDataSource`：按规则生成树（如 `plant{0..9}.line{0..9}.sensor{0..999}` = 10w leaf），值产生器按 item 类型（counter/sine/random）。**用于压测与功能验证**。
- 生产数据源（协议网关）后续按需，本方案不实装。

**改造点**：
1. `data_source.rs`：`NamespaceTree` 树化 + `DataSource` trait 加 `item_range`/`browse_branch`；新增 `GeneratedDataSource`。
2. `server.rs`：`ServerObj` 加 `browse_pos`；重写 `IOPCBrowseServerAddressSpace_Impl` 5 方法（按上表）。
3. `SimDataSource` 适配新 trait（`item_range` 返 `None`，`browse_branch` 返单层 4 leaf 的"flat as branches" 或保持 flat 语义供回归）。
4. `StringEnum`（`browse.rs`）不变（已支持任意 Vec<String> 快照）。

**验证标准**：
- [ ] 单测：`GeneratedDataSource` 10w leaf，树深度/广度正确；`index` O(1) read/item_meta。
- [ ] 单测：browse 状态机——`DOWN("plant0").DOWN("line0").Browse(LEAF)` 返回该分支 sensor；`UP` 回退；`TO("/plant0")` 跳转。
- [ ] e2e：opc-da-client-test browse 探针扩展，验证 hierarchical 导航（ opc-da-client 的 browse 已支持 hierarchical 递归，见 CLAUDE.md §4 Browse 策略）。
- [ ] P4(v3)：browse 10w item 全枚举延迟（应秒级内），分批 Next 稳定。

**工作量**：3–5 天（browse 状态机 + 树构造 + 测试是大头）。

**风险**：
- **R5（browse_pos 并发）**：`IOPCBrowseServerAddressSpace` 在 ServerObj 上，多 client 并发 browse 共享 `browse_pos` 会串。**OPC DA 规范**：browse position 是 per-ServerObj-instance 的。但当前 ServerObj 单例（一个 server 一个）。正确做法：browse 状态应 per-调用者或返回独立 enumerator。**需 PoC**：实测标准 client 是否串行 browse。缓解：`browse_pos` 用 `Mutex`（串行化 browse 调用，可接受因 browse 非高频）或每 client 独立 browse session。
- **R6（SimDataSource 兼容）**：现有 13 探针 + 单测依赖 SimDataSource 的 flat 行为。改造时保留 SimDataSource flat 语义（QueryOrganization 仍 FLAT，或 GeneratedDataSource 作 hierarchical、SimDataSource 作 flat 回归）。

**决策点**：
- browse_pos 串行（`Mutex`）vs per-client session。先 `Mutex`（简单），压测发现 browse 成瓶颈再改。

---

### P3 — 锁与分配优化（按压测结果定向）

**目标**：消除推送路径的分配压力，榨吞吐。**按 P4 压测发现的真瓶颈做**，避免过早优化。

**子项**（按性价比排序）：

#### P3.1 item_id: Arc\<str\>（确定做，P1 时一并）
`ItemEntry.item_id: String` → `Arc<str>`；snapshot 返回 `Vec<(u32, Arc<str>)>`。clone `Arc` = 原子计数，无堆分配。
- 改造：`group.rs:83`、`group.rs:117-137` snapshot。
- 验证：单测 snapshot 不分配（或分配数下降）。

#### P3.2 推送缓冲区复用（压测后定）
worker 线程 thread-local 持 5 个 `Vec`（hclients/values/qualities/timestamps/errors），每帧 `clear()` + reuse，不 new。
- 改造：`worker::push_one` 用 `thread_local!` buffer。
- 验证：压测 allocator 调用数下降。

#### P3.3 VARIANT 池（压测后定，谨慎）
高频场景池化 VARIANT。VARIANT 含 union，池化要 `VariantClear` 清理。**仅在 P3.1/3.2 后仍分配瓶颈时做**。
- 风险：VARIANT 生命周期管理复杂（COM 所有权），易引入 UB。优先级最低。

#### P3.4 ServerObj groups 锁（按需）
`ServerObj.groups: HashMap` 的锁在 AddGroup/RemoveGroup（建连期）竞争。100+ client 并发建连时可能争。
- 缓解：`RwLock`（读多写少）或分片锁。**仅 P4 发现建连瓶颈时做**。

**工作量**：P3.1 随 P1（0 额外）；P3.2 0.5–1 天；P3.3 1–2 天（高风险）；P3.4 0.5 天。

---

### P4 — 压测基础设施与达标验证

**目标**：建立可量化的压测手段，验证目标达标 + 找瓶颈。**每阶段后跑对应版本**。

**设计**：benchmark bin（`opc-da-server/examples/stress.rs` 或独立 crate）。

```rust
// 压测工具职责
// 1. 起 server（GeneratedDataSource, N item，可配）
// 2. 起 M 个 mock client（每 client K 组 × L item 订阅，可配）
// 3. mock client 统计 OnDataChange 收到的 item/s（计数器）
// 4. 输出指标：item/s、OnDataChange 调用/s、延迟 p50/p95/p99、CPU%、内存、线程数
```

**mock client**：复用 `opc-da-client`（已有 `subscribe`），多线程 spawn M 个 `OpcDaClient`，每个 advise + 计数回调频率。

**压测矩阵**：

| 版本 | 场景 | 达标线 |
|---|---|---|
| **v1**（P0 后）| 1w 组 / 100 client / 各 100 item，deadband=0 | 线程数 ≤ 核数×2+常数；推送稳定无丢；持续 60s 无 OOM |
| **v2**（P1 后）| 同 v1 + deadband 5% + 30% 变化率数据 | OnDataChange 调用数 vs deadband=0 降 1 个量级；item/s 提升 |
| **v3**（P2 后）| 10w item 树 / 100 client / 总订阅 50w（部分订阅）/ deadband 5% | browse 10w 全枚举 < 2s；推送 item/s 达标；100 client 并发稳定 60s |
| **余量** | 逐步加到 100w 总订阅 / 200 client | 找到软肋（线程/内存/吞吐）|

**远程压测**（可选，需 DCOM 配置）：100 client 跨机订阅，验证反向回调配置（已知坑，CLAUDE.md 记录）。

**指标采集**：
- Rust 侧：原子计数器（item/s、OnDataChange/s）。
- 系统侧：`GetProcessHandleCount`/线程数、`GlobalMemoryStatus`/RSS、perfmon 计数器。
- 延迟：OnDataChange 内时间戳 vs DataSource::read 时间戳之差。

**工作量**：2–3 天（含 mock client + 指标采集 + 矩阵跑批）。

**验证标准**：v3 达标 = 本方案目标完成。余量测试输出瓶颈报告，指导 P3。

---

## 5. 关键数据结构与接口（汇总）

```rust
// === publisher/scheduler.rs ===
pub struct Scheduler { /* §P0 */ }
struct Bucket { jobs: Vec<Arc<PublishJob>>, next_tick: Mutex<Instant> }
pub struct PublishJob { key, inner, data_source, data_cp, rate }
struct PublishTask { job: Arc<PublishJob> }

// === publisher/worker.rs ===
pub fn push_one(job: &PublishJob);              // 含 P1 deadband 循环
fn should_push(...) -> bool;                     // P1
pub fn enumerate_sinks(...) -> Vec<IOPCDataCallback>;  // 已有，复用
pub fn push_data_change(..., trans_id: u32);     // 已有，复用

// === group.rs ===
struct ItemEntry { item_id: Arc<str>, h_client, active, data_type,
                   last_pushed: Mutex<Option<PushState>> }  // P1
struct PushState { value: f64, quality: u16, ts_serial: u64 } // P1
pub struct GroupInner { /* +group_key: GroupKey */ }
impl GroupObj { /* new: 注册 Scheduler; Drop: 注销 */ }

// === data_source.rs ===
pub enum NsNode { Branch{name, children}, Leaf{id, meta, range} }  // P2
pub struct NamespaceTree { root, index, leaves_flat }
pub trait DataSource { /* +item_range, +browse_branch */ }          // P1+P2
pub struct GeneratedDataSource { /* 10w item 压测数据源 */ }        // P2
pub struct SimDataSource { /* 保留，flat 回归 */ }
```

---

## 6. 风险登记表

| ID | 风险 | 阶段 | 缓解 |
|---|---|---|---|
| R1 | Scheduler job 注销与 worker 执行竞态 | P0 | `Arc<PublishJob>` 引用计数；Drop 仅移除 registry，不终止执行中 job |
| R2 | worker 线程未进 MTA，COM 调用失败 | P0 | worker 启动 `CoIncrementMTAUsage`（复用 `publisher.rs:61`）|
| R3 | 非数值 VARIANT（VT_BSTR 等）无 deadband 语义 | P1 | 退化成"值变化即推"，文档化 |
| R4 | item EU 范围未知，deadband 失效 | P1 | `DataSource::item_range` 返 `None` → 退化任意变化即推 |
| R5 | browse_pos 多 client 串扰 | P2 | `Mutex` 串行化（browse 非高频）；PoC 实测后定是否 per-session |
| R6 | SimDataSource flat 行为被 13 探针/单测依赖 | P2 | SimDataSource 保留 flat；GeneratedDataSource 做 hierarchical |
| R7 | 远程 100+ client DCOM 反向回调配置 | P4 | 运行时配置（client 入站 DCOM + sink 编组），非代码；本机测不暴露 |
| R8 | 压测 mock client 自身成瓶颈 | P4 | mock client 跨多机或限制 client CPU；指标在 server 侧采 |

---

## 7. 决策点（实现时拍板）

| 决策 | 选项 | 默认 | 触发评审 |
|---|---|---|---|
| D1 worker 池实现 | 自建 std / rayon / tokio | **自建 std** | P0 实现前；P4 压测对比 |
| D2 调度 tick 精度 | 1ms / 自适应 | **1ms** | P0；OPC min update_rate=10ms |
| D3 browse_pos 串行 vs per-session | Mutex / per-client | **Mutex** | P2；PoC 后定 |
| D4 DataSource 配置方式 | 代码生成 / 配置文件 / 插件 | **GeneratedDataSource（压测）** | P2；生产数据源另案 |
| D5 P3.3 VARIANT 池 | 做 / 不做 | **不做**（除非压测强制）| P3 压测后 |

---

## 8. 实现执行规程

每阶段按以下步骤（每阶段独立 commit）：

1. **设计复核**：读本文件对应 P 章节 + 引用的现有代码，确认改造点。
2. **实现**：按"改造点"逐条改；每个 `unsafe` 块写 `// SAFETY:` 注释（项目约定）。
3. **单测**：按"验证标准"单测项先写，跑通（TDD 意图驱动）。
4. **质量门**：`cargo fmt --check` + `cargo clippy -p opc-da-server --all-targets -D warnings` + `cargo test -p opc-da-server` 全过。
5. **e2e**：`pwsh -File scripts/verify.ps1`（含 13 探针，确保无回归）。
6. **commit**：`pwsh -File scripts/commit.ps1 -Message "feat/fix/perf(opc-da-server): <阶段> <内容>"`。
7. **勾选**：更新本文件 §4 对应阶段 checklist（`[ ]`→`[x]`）+ commit 本文件。
8. **压测**（P4 节点）：跑对应版本压测矩阵，结果记入 §9。
9. **检查点**：阶段末向用户汇报——已完成/已验证/剩余 + 下一阶段。

**遇阻**：unsafe 坑 / 签名折腾 > 3 次 / 设计不清 → 停，写 `LOOP_STATUS.md`，向用户报告。

**绝不**：commit 未过 verify 的代码；为 server 改 client 逻辑（client 真 bug 除外）。

---

## 9. 进度跟踪 Checklist

### P0 — 统一推送调度（`cae5f13`，完成）
- [x] 新增 `objects/scheduler.rs`（Scheduler + PublishJob + tick/worker 线程）；`publisher.rs` 精简为推送数据函数（`enumerate_sinks`/`push_data_change`）。未拆 `publisher/` 目录——scheduler.rs 含调度+worker、publisher.rs 含数据函数，职责已分。
- [x] `Scheduler` 数据结构 + 时间轮（按 rate 分桶）+ N worker 线程池（纯 std：`Mutex<VecDeque>`+`Condvar` MPMC，零新依赖）
- [x] `PublishJob` + GroupKey（=`h_server_group`）注册/注销
- [x] `GroupObj::new` 改注册 Scheduler；`GroupObj::Drop` 注销
- [x] `bin/opc-da-server.rs` init 全局 Scheduler（workers = 核数）
- [ ] ~~单测：1000 组线程数稳定~~ → 移 **P4**（线程数需进程级查询，压测时验）
- [x] 单测：job 注册/注销（`scheduler_register_unregister_updates_count`，含多桶 + 幂等）
- [x] verify 全门 + 13 探针无回归（subscribe 探针验证统一调度推送实际工作）
- [x] commit + 勾选

### P1 — deadband 变化检测（`ce81148`，完成）
- [x] `ItemEntry` + `last_pushed: Option<PushState>` + `PushState{value:f64, quality:u16}`
- [x] `item_id: Arc<str>`（P3.1 并入，snapshot/推送路径避免 String clone）
- [x] `should_push` + `normalize_variant`（VT_I4/R8/I2/BOOL → f64）
- [x] worker `push_one` 加 deadband 循环（锁内 read + should_push + 更新 last_pushed + 收集变化帧）
- [x] `DataSource::item_range` trait 扩展（默认 `None`，SimDataSource 继承）
- [x] Refresh2 保留全推路径（绕过 deadband，`read_frames` + push_data_change）
- [x] 单测：`should_push_deadband_semantics`（各分支）+ `normalize_variant_numeric_types`；e2e subscribe 验证 deadband=0 全推回归
- [x] verify 全门
- [x] commit + 勾选

### P2 — hierarchical + 10w item（P2.1 `e88c83e` + P2.2 `eba8f06` + P2.3 `0df5a2f`）
- [x] `NamespaceTree` 树化（NsNode + index + leaves_flat）—— P2.1：`NsNode::Branch/Leaf` + `collect_leaves` 扁平化（无显式 HashMap index；read 经数据源内部 `HashSet` O(1)）
- [x] `DataSource` trait：`browse_branch` + `item_range` —— P2.1 `browse_branch` + P1 `item_range`
- [x] `GeneratedDataSource`（10w item 树构造）—— P2.2（`plant×line×sensor` 规则树，`new(10,10,1000)`=10w leaf）
- [x] `SimDataSource` 适配（flat 保留）—— `query_organization` 默认 `Flat`，namespace 单层 leaves
- [x] `ServerObj` 加 `browse_pos` + 重写 browse 5 方法 —— P2.3：`QueryOrganization` 取 ds 类型；`ChangeBrowsePosition` 维护 `browse_pos`（DOWN/UP/TO）；`BrowseOPCItemIDs` hierarchical 返相对名（client 经 `GetItemID` 转 full id）/ flat 返 full id / `OPC_FLAT` 跨分支全量；`GetItemID` 相对名拼 full path
- [x] 单测：树构造 + browse 状态机 + 10w item O(1) 查找 —— `generated_data_source_tree_and_read`（树构造）+ `browse_hierarchical_*` 7 测（DOWN/UP/TO/BRANCH/LEAF/FLAT/GetItemID 状态机）+ `browse_flat_sim_namespace_unchanged` 回归；`HashSet` O(1) 查找
- [x] opc-da-client-test browse 探针扩展（hierarchical）—— P4.1 完成（`d0428cc` server env 切 ds + `4ab83ad` client-test e2e）：spawn GeneratedDataSource server + browse_children 下钻 plant0→line0→sensor，验证 GetItemID 相对名→full path（实测 17 passed）
- [x] verify 全门 —— 13 探针无回归（browse flat 探针仍过）
- [x] commit + 勾选 —— P2.3 feat `0df5a2f` + 本 chore

### P3 — 优化（按压测）
- [ ] P3.1 `Arc<str>`（随 P1）
- [ ] P3.2 thread-local buffer（P4 发现分配瓶颈后）
- [ ] P3.4 ServerObj RwLock（P4 发现建连瓶颈后）
- [ ] P3.3 VARIANT 池（仅强制时）

### P4 — e2e + 压测（P4.1 e2e 全流程完成 `d0428cc`+`4ab83ad`；P4.2 stress 待做）
- [x] P4.1 e2e 载体：server env 切数据源 + client-test 模块化（server_proc/e2e/report）+ 13 flat + hierarchical 探针（实测 17 passed，verify 全门过）
- [ ] stress 工具骨架（起 server + M mock client + 指标采集）
- [ ] mock client（复用 opc-da-client subscribe + 计数）
- [ ] v1 矩阵（P0 后）：1w 组 / 100 client / 线程数
- [ ] v2 矩阵（P1 后）：deadband 效果
- [ ] v3 矩阵（P2 后）：10w item + 50w 订阅达标
- [ ] 余量测试 + 瓶颈报告
- [ ] 压测结果记入 §10

---

## 10. 压测结果记录（实现时填）

| 版本 | 日期 | commit | 场景 | item/s | OnDataChange/s | 延迟 p99 | 线程数 | 内存 | 结论 |
|---|---|---|---|---|---|---|---|---|---|
| v1 | — | — | — | — | — | — | — | — | 待 P0 |
| v2 | — | — | — | — | — | — | — | — | 待 P1 |
| v3 | — | — | — | — | — | — | — | — | 待 P2 |

---

## 11. 里程碑

- **M-Scale-1**（P0 完成）：统一调度落地，1w 组线程数稳定 → 解锁规模化的前置。
- **M-Scale-2**（P1 完成）：deadband 生效，推送量可控。
- **M-Scale-3**（P2 完成）：hierarchical + 10w item 功能达标。
- **M-Scale-4**（P4 v3 达标）：100+ client / <100w 订阅 / 10w item 树，目标达成。

预计总工作量：**8–14 人日**（P0 1.5-2 + P1 1-2 + P2 3-5 + P3 1-2 + P4 2-3，含调试冗余）。

---

## 附录 A：与上游设计的映射

- 本文件的 `PublishJob`/`Scheduler` 对应上游设计 §10（订阅推送引擎）的**修订**：上游 §10 描述"per-group thread"是阶段 1 MVP 实现，本文件将其升级为统一调度（阶段 5 规模化）。
- `DataSource` trait 扩展（`item_range`/`browse_branch`）对应上游 §9（数据源抽象）的**增强**。
- hierarchical browse 对应上游 §8（ConnectionPoint）之外的 `IOPCBrowseServerAddressSpace` 完整实装（上游 §2 阶段规划）。

实现中如发现本文件与上游设计冲突，**以本文件为准**（本文件是 2026-08-03 最新规划），并在 commit 说明冲突点。
