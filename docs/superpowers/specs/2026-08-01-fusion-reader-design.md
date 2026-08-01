# FusionReader 库化 — 融合读取接口（默认订阅 + 同步兜底）

- 日期: 2026-08-01
- 状态: 设计已对齐
- 关联: [`verify_dcom_auth.rs`](../../opc-da-client/examples/verify_dcom_auth.rs)（逻辑已真机验证，已合入 main）、[`2026-07-31-dcom-auth-fusion-design.md`](./2026-07-31-dcom-auth-fusion-design.md)

## 背景

`examples/verify_dcom_auth.rs` 的 `fusion_reader` 已真机验证"订阅优先 + 同步兜底"逻辑（已合入 main）：远程订阅反向回调不通时 `0x800706BA`，自动 fallback 到同步轮询并连续读到数据流。本次将其**提取为 `opc-da-client` 的 pub API**，供上层应用（opc-cli / opc-da-desktop / 第三方）复用。

## 目标

1. `FusionReader` struct：默认订阅（`OnDataChange` 推送）；订阅失败 / 超时 / 回调静默 / 流关闭 → 自动 fallback 同步轮询。
2. 显式 DCOM 凭据：`Option<AuthCredentials>`，`None` = 当前登录用户，`Some` = 显式 user/password。
3. 事件单流 `FusionEvent`，上层感知数据 + fallback + 订阅状态。

## 非目标（本次不做）

- 不改 `opc-cli` / `opc-da-desktop`（只库化，上层后续自行采用）。
- 不改 `OpcProvider` trait（`FusionReader` 是独立 struct，内部用 `OpcDaClient`）。
- 不改订阅 sink 的 `CoSetProxyBlanket` / 进程级 `CoInitializeSecurity`（远程订阅回调仍取决于 client 端 DCOM 配置；不通则由 fallback 兜底）。

## 设计

### 模块

`opc-da-client/src/fusion_reader.rs`（pub，经 `lib.rs` 导出）。

### 类型

```rust
pub struct FusionReader { /* 后台 task handle + 取消句柄 */ }

pub struct FusionReaderOptions {
    pub update_rate: u32,           // ms，默认 1000
    pub fallback_timeout: Duration, // 默认 10s
    pub buffer: usize,              // 事件 channel 容量，默认 256
}

pub enum FusionEvent {
    Data(TagValue),
    Subscribed,        // 订阅建立成功，进入推送模式
    Fallback(OpcError),// 切同步兜底（订阅失败 / 超时 / 静默 / 流关闭），携带原因
}
```

### API

```rust
impl FusionReader {
    /// 启动融合读取。返回 (FusionReader, 事件接收端)。
    /// 必须在 tokio runtime 内调用（内部 spawn 后台 task）。
    pub fn start(
        host: &str,
        creds: Option<AuthCredentials>,
        server: &str,
        tags: Vec<String>,
        opts: FusionReaderOptions,
    ) -> OpcResult<(Self, tokio::sync::mpsc::Receiver<FusionEvent>)>;
}
// Drop → unadvise 订阅 + abort 后台 task（避免泄漏 COM worker）。
```

### 内部逻辑（照搬 example，已验证）

1. 据 `creds` 建 `sub_client` + `read_client`（两个 `OpcDaClient`，各自独立 COM worker，互不阻塞）：
   - `None` → `OpcDaClient::new(ComConnector::new(host))`（当前登录用户）。
   - `Some(c)` → `OpcDaClient::with_credentials(host, c)`。
2. `tokio::spawn` 后台 task：
   - `sub_client.subscribe(server, tags, update_rate)`，**限内部 timeout（8s）**——远程反向回调不通时 server 端 Advise 会长时间 RPC 超时，必须截断。
   - 成功 → 发 `Subscribed` + 推送模式（drain 订阅 `rx`，`tx.send(Data)`）；静默超 `fallback_timeout` → 发 `Fallback` + 切同步。
   - 失败 / 超时 / 推送流关闭 → 发 `Fallback` + 切同步。
   - 同步模式：`read_client.read_tag_values(server, tags)` 按 `update_rate` 周期轮询，`tx.send(Data)`。
3. 事件顺序：`Subscribed`/`Fallback` 先于 `Data`（上层先知模式）。

## 验证标准

1. `cargo build --workspace` + `make verify`（fmt/clippy/test/compat）通过。
2. 单测：`FusionReaderOptions::Default` 默认值；fallback 触发逻辑（mock subscribe 失败 → Fallback 事件）。
3. `example verify_dcom_auth.rs` 改用 `FusionReader`（替换内部 `fusion_reader` 函数），真机验证（订阅回调不通 → Fallback → 同步 read 数据流，与之前一致）。
4. 现有 lib(75)/doc/e2e 测试不破坏。

## 实现顺序

1. `fusion_reader.rs`：`FusionReader` + `FusionEvent` + `FusionReaderOptions` + `start` + `Drop`。
2. `lib.rs`：pub 导出。
3. `example`：改用 `FusionReader`（验证 + 演示）。
4. `make verify` + 真机验证。
5. 文档：`DCOM_GUIDE.md` §5 增 `FusionReader` 库 API 说明；更新 README/features（可选）。

## 风险

- `FusionReader::start` 需 tokio runtime（spawn）；上层须在 `#[tokio::main]` / runtime 内调用。文档注明。
- subscribe 反向回调不通时 8s timeout 是经验值（太短误判、太长阻塞），不作为 `FusionReaderOptions` 字段（内部常量）。
- `Drop` 取消：unadvise + abort task 要正确，避免泄漏 COM worker 线程。
- 事件 channel 满（上层 drain 慢）：`tx.try_send` 丢最旧 Data（保 Subscribed/Fallback），避免 task 阻塞。
