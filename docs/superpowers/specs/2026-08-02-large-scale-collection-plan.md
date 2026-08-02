# 大数据量采集改进方案（10W 标签实时订阅）

> 状态：方案（未实现）。基于 2026-08-02 代码分析。
> 目标：支持单/多 DA Server 10W 标签点实时订阅（多连接 + 多订阅组）。
> 前提：DA Server 无问题（够强、允许多 session）。

## 背景：现状瓶颈（前文研究结论）

| 瓶颈 | 位置 | 影响 |
|---|---|---|
| **`channel(256)` + `try_send` 静默丢** | `com_worker.rs:2143` + `subscription.rs:173` | 10W 标签 burst 瞬间填满 → 静默丢弃（`let _ = try_send`，丢了不知道） |
| **每回调 5 个 String 分配** | `subscription.rs:165` `forward_data_change` | `tag_id.clone()` + `variant_to_string` + `variant_type_name` + `quality_to_string` + `filetime_to_string`，10W 标签/周期 = 50W String/周期 |
| **单 connection 单 COM session** | `AppState` 单 `OpcDaClient` | server 侧单订阅流上限，无法横向扩展 |
| **多组同 connection** | `ComWorker` 连接池 | 多组分担 channel，但同 connection、同 server session |

**COM 线程模型**（有利）：`OnDataChange` 回调在 COM MTA 的 RPC 线程（多线程并发），**不是单线程瓶颈**；CPU 多核可利用。真正瓶颈是 channel 丢 + 字符串分配 + 单 connection。

---

## 一、基础库改动（opc-da-client）

### P1：channel 容量可配 + 丢包可见【小改，立即收益】

**问题**：`channel(256)` 写死 + `try_send` 失败 `let _ =`（静默丢）。

**改动**：
- `OpcProvider::subscribe` 签名加 `channel_capacity: usize`（或封装 `SubscriptionOptions { update_rate, channel_capacity, fallback_timeout }`，避免参数膨胀）：
  ```rust
  // provider.rs
  async fn subscribe(
      &self, server: &str, tag_ids: Vec<String>, update_rate: u32, channel_capacity: usize,
  ) -> OpcResult<SubscriptionHandle>;
  ```
  （破坏性——但 subscribe 消费者少：opc-cli TUI、desktop、e2e、example。一次性改。或加 `subscribe_with_opts` 新方法 + 旧 `subscribe` 走 default capacity 256，零破坏。**推荐后者**。）
- `com_worker.rs:2143` `mpsc::channel(256)` → `mpsc::channel(channel_capacity)`
- `ComRequest::Subscribe` 变体加 `channel_capacity` 字段
- `DataCallbackSink` 加 `dropped_count: Arc<AtomicU64>`；`subscription.rs:173`：
  ```rust
  match tx.try_send(tv) {
      Ok(()) => {}
      Err(mpsc::TrySendError::Full(_)) => { dropped_count.fetch_add(1, Ordering::Relaxed); }
      Err(mpsc::TrySendError::Closed(_)) => {}
  }
  ```
- 周期性 `tracing::warn!` 丢包速率（如 health monitor 每 N 秒 log 一次 `dropped_count` 增量）
- `SubscriptionHandle` 加 `dropped_count: Arc<AtomicU64>` + `pub fn dropped_count(&self) -> u64`

**收益**：channel 可按负载调（1024/4096/...）；丢包可见可观测，不再静默。
**风险**：低。default 256 保持旧行为。

---

### P2：减字符串分配（`tag_id` 改 `Arc<str>`）【中改，减分配】

**问题**：`forward_data_change` 每条回调 `tag_id.clone()`（String 分配）。

**改动**：
- `DataCallbackSink.tag_ids: Vec<Arc<str>>`（`add_items` 时建一次，`com_worker.rs:2019`）
- `TagValue.tag_id: Arc<str>`（`provider.rs`）—— clone 变原子加，无分配
- `forward_data_change`：`tv.tag_id = Arc::clone(tag_ids.get(handle)?)`
- `variant_to_string` / `variant_type_name` / `quality_to_string` / `filetime_to_string` **保留**（VARIANT 在回调后释放，不能持 raw；转换必要）—— 但可单独优化（如 `quality_to_string` 返回 `&'static str`，已可能；`variant_to_string` 复用缓冲）

**收益**：每回调省 1 String 分配（10W/周期 省 10W 分配）。
**风险**：中。`TagValue` 是公开类型，`tag_id: String → Arc<str>` 破坏消费者（desktop 的 `TagUpdate::from(TagValue)`、TUI）。一次性改 + 重导出。

---

### P3：批消息（`Vec<TagValue>` 一条）【中改，减 channel 开销】

**问题**：`forward_data_change` 逐条 `try_send`（10W 标签 = 10W 次 channel 操作）。

**改动**：
- channel 类型 `mpsc::Sender<TagValue>` → `mpsc::Sender<Vec<TagValue>>`
- `forward_data_change` 攒一批 `Vec<TagValue>`（一次 `OnDataChange` 的 dwcount 条），末尾 `try_send(batch)`
- consumer（`subscription_runner` / `fusion_runner`）recv `Vec<TagValue>` → 循环 `channel.send(TagUpdate)`
- 配合 P1 丢包计数：批 send 失败计 `batch.len()`

**收益**：channel send 次数从"每标签 1 次"降到"每回调批次 1 次"（10W → ~数百批）。channel 内部锁竞争大减。
**风险**：中。consumer 接口变（recv Vec）。与 P2 合并做最划算。

---

### P4：多 client 横向扩展【库无需改，上层负责】

`OpcDaClient` 已支持多实例（每实例独立 `ComWorker` + connection）。库侧**无改动**。上层（desktop）做标签分片 → N 个 `OpcDaClient` 各持一组标签。

**库侧可选增强**（低优先）：
- 加一个 `OpcDaClientPool` 工具类（封装 N 个 client + 标签分片 + 负载均衡），供上层复用。非必需。

---

## 二、桌面程序改动（opc-da-desktop）

### M1：多连接管理【大改，scale 主力】

**现状**：`AppState` 单 `client: Mutex<Arc<OpcDaClient>>`（`state.rs`）。所有操作经单 client。

**改动**：
- `AppState` 加 `clients: Mutex<HashMap<ConnId, Arc<OpcDaClient>>>`（每连接独立 host/creds/ComWorker）
- 数据面 commands（read/write/browse/subscribe）加 `conn_id` 参数，路由到对应 client
- `ServerPanel` 改多连接：当前单 host/creds → 连接列表 + 新增连接（每连接独立 host/creds/server）
- `set_host` → `add_connection(host, creds)` / `remove_connection(conn_id)`
- 连接 panel 显示多连接状态

**收益**：10W 标签拆 N 连接（如 10×1W），CPU + channel + COM session 全分布。
**风险**：高。`AppState` 核心 + 全部 command 签名 + UI 重构。建议分阶段（先支持多连接，再分片）。

---

### M2：标签分片【中改，依赖 M1】

**改动**：
- 订阅时若标签数 > 阈值（如 1W），自动拆分到多连接（round-robin 或按 server）
- 或手动：每个 group 指定 connection（UI 选）
- group → connection 映射存 `AppState`

**收益**：单大订阅自动分布多连接。
**风险**：中。分片策略 + 跨连接 group 聚合（UI 显示）。

---

### M3：订阅配置暴露【小改，配合 P1】

**改动**：
- `GroupState` 加 `channel_capacity`（UI 可配，默认 256/1024）
- `subscribe_tags` / `subscribe_fusion_tags` command 加 `channel_capacity` 参数 → 透传库 P1
- `GroupEditor` 加 channel_capacity 输入（高级选项，默认隐藏）

**收益**：用户可按负载调 channel。
**风险**：低。

---

### M4：丢包监控【小改，配合 P1】

**改动**：
- `SubscriptionCreated` 返回 + runner 周期回读 `dropped_count`（或新 command `get_subscription_stats(conn_id, cookie)`）
- `GroupState` 加 `dropped: u64`，UI 显示"⚠ 丢弃 X 条"
- 丢包率超阈值标红

**收益**：大数据量时用户能看到丢包（不再静默）。
**风险**：低。

---

### M5：大数据量 UI 优化【中改，React 性能】

**问题**：`TagTable` 10W 行 + 实时更新 → React setState 频繁（每标签一个 update）。

**改动**：
- **batch setState**：`subscription_runner` 攒一批（如 50ms 窗口）再 patch rows（现在每条 onmessage 一次 set）
- **按 group 分表 / 滚动窗口**：只渲染可视区 + 附近（`@tanstack/react-virtual` 已用，但要确认 10W 行 + 实时 patch 的 perf）
- **节流更新**：UI 更新频率上限（如 10Hz），后台 batch
- `rows: Map` patch 批量（`patchGroup` 一次多标签）

**收益**：10W 行 UI 不卡。
**风险**：中。React 渲染优化需实测。

---

## 三、验证：benchmark【必做，否则"能否 10W"只能猜】

**目标**：量化吞吐 / 延迟 / 丢包率，找真实瓶颈。

**方案**：用 `ConfigurableMockConnector`（`com_worker.rs` tests 已有）扩展，模拟大量标签回调：
- mock `OnDataChange` 按指定 rate（如 1s）推送 N 标签（1K/1W/5W/10W）
- 测：consumer drain 速率、channel 满丢包率、CPU、延迟（回调 → consumer recv）
- 对比 P1/P2/P3 改动前后

**新增**：`opc-da-client/benches/subscribe_throughput.rs`（criterion）。mock 驱动，不依赖真实 server。

---

## 四、落地优先级（路线图）

| 阶段 | 改动 | 收益 | 工作量 |
|---|---|---|---|
| **P1** | channel 可配 + 丢包可见 | 立即可观测，避免静默丢 | 小 |
| **benchmark** | mock 大量标签测压 | 量化瓶颈，验证后续 | 中 |
| **P2 + P3** | Arc<str> + 批消息 | 减分配 + channel 开销 | 中 |
| **M3 + M4** | 桌面订阅配置 + 丢包监控 | 用户可调可观测 | 小 |
| **M1** | 多连接管理 | scale 主力 | 大 |
| **M2** | 标签分片 | 自动分布 | 中 |
| **M5** | UI 大数据量优化 | 10W 行不卡 | 中 |

**建议顺序**：P1 → benchmark（验证 P1 效果 + 量化瓶颈）→ P2/P3 → M3/M4 → M1/M2 → M5。

---

## 五、预期效果（改完 + 多连接）

- 单 client：1W-2W 标签 @ 1Hz 稳定（P1/P2/P3 后），channel 不丢
- 10 连接 × 1W 标签 = 10W：CPU 10 核分布，每连接 channel 独立，server 侧 10 session 并行
- 丢包可见（M4），用户可调 channel_capacity（M3）+ 连接数（M1）平衡

**前提**：DA server 允许多 session + 处理多订阅流；client 机器 CPU/内存够（10W 标签 × N connection 的 COM 线程 + channel 缓冲）。
