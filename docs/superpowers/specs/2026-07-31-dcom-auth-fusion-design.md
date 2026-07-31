# DCOM 凭据接通 + 融合读取验证 — 设计

- 日期: 2026-07-31
- 状态: 设计已与作者对齐（三项决策采纳推荐）
- 关联: [`../../DCOM_GUIDE.md`](../../DCOM_GUIDE.md) §5.4

## 背景

`opc-da-client` 远程 DCOM 激活硬编码 `pAuthInfo: null`（用当前登录用户）。`AuthInfo`/`AuthIdentity`/`COAUTHIDENTITY` 类型齐全但**链路断开**：`create_server2`（`client.rs:152`）与 `get_servers`（`client.rs:38`）丢弃 `auth_info`；`ServerInfoBridge/AuthInfoBridge::try_to_native()`（`typedefs.rs:628/703`）返回临时 `COAUTHINFO` 再取址 → 悬垂指针（曾致 32 位 `0x800703E6`，目前是死代码潜伏）。后果：**无法手动指定访问远程 DA Server 的用户名/密码**。

本机当前用户 `ncpepc` ≠ 远程期望用户 `viccom`，正好作为验证凭据价值的对照。

## 目标

1. **想法1**：接通 DCOM 凭据链路，支持手动指定 `user/password/domain` 访问远程 server。
2. **想法2**：上层融合读取（订阅优先 + 同步兜底），应对 NAT/防火墙/client 端 DCOM 难配场景。
3. 用真实环境（`192.168.199.155` / `viccom` / `Matrikon.OPC.Simulation.1` / `Random.Real4`）验证。

## 非目标（本次不做）

- 不改订阅 sink 的 `CoSetProxyBlanket`、不改进程级 `CoInitializeSecurity`。
- 不动 TUI。
- 不改 `OpcProvider` trait 方法签名（凭据在 client 构造层注入）。

## 设计

### P1 — 接通 AuthInfo 链路

**① 修 Bridge 悬垂指针**（`opc_da/typedefs.rs`）
让 `AuthIdentityBridge` 持有 `native: COAUTHIDENTITY`、`AuthInfoBridge` 持有 `native: COAUTHINFO`，在 `into_bridge()` 时构造，字段指针引用 Bridge 自身已有的 `LocalPointer<Vec<u16>>` wide string。`ServerInfoBridge::try_to_native()` 返回的 `COSERVERINFO.pAuthInfo` 指向 `&self.auth_info.native`（稳定地址）。三层（`COSERVERINFO`→`COAUTHINFO`→`COAUTHIDENTITY`→strings）全由 Bridge owned，生命周期覆盖 `CoCreateInstanceEx`。

**② `create_server2` / `get_servers` 用凭据**（`opc_da/client/traits/client.rs`）
- `create_server2` 的 `Some(ServerInfo)` 分支：`let bridge = info.into_bridge(); let native = bridge.try_to_native()?;`，Bridge 局部变量存活覆盖 `CoCreateInstanceEx`。
- **空凭据兼容**：`AuthIdentity.user` 为空 → `pAuthInfo: null`（当前用户，完全向后兼容）；非空 → 用 Bridge 凭据。
- `get_servers` 签名从 `host: Option<&str>` 扩展为额外接收 `auth: Option<&AuthInfo>`；`ComConnector::enumerate_servers` 传入。

**③ 凭据注入 API**（`backend/connector.rs` / `lib.rs` / `opc_da.rs`）
- 新增 pub `AuthCredentials { user: String, password: String, domain: Option<String> }`；`Debug` 屏蔽密码（写 `"***"`）；`lib.rs` 导出。
- `ComConnector` 加 `credentials: Option<AuthCredentials>`；`new(host)` = `None`，新增 `with_credentials(host, creds)`。
- `helpers::connect_server` 加 `credentials: Option<&AuthCredentials>` 参数，构造对应 `AuthInfo`（非空凭据用 `RPC_C_AUTHN_LEVEL_CONNECT` + `RPC_C_IMP_LEVEL_IDENTIFY` + 填 `AuthIdentity`；空则 `default_dcom`）。
- `OpcDaClient::with_credentials(connector, creds)`；现有 `new`/`default` 保持（`None`）。
- `OpcProvider` trait **签名不变**。

### P2 — 融合读取接口（写在 example，不入库）

**`FusionReader`**（`examples/verify_dcom_auth.rs` 内）
- 输入：`client`、`server`、`tags`、`update_rate`、`fallback_timeout`（默认 10s）。
- 逻辑（**两者都要** fallback）：
  1. `try subscribe(tags, update_rate)`。
  2. 失败（建组/Advise/加项）→ 立即进同步轮询，记录原因 `SUBSCRIBE_SETUP_FAILED`。
  3. 成功 → 订阅模式 drain `rx`；并发定时器，`fallback_timeout` 内无 `OnDataChange` → 切同步轮询，记录 `CALLBACK_SILENT`。
  4. 同步轮询：周期 `read_tag_values`（用 `update_rate`）。
- 输出：每条数据标注 `SUBSCRIBE` / `SYNC_FALLBACK` + 触发原因。

**`examples/verify_dcom_auth.rs`**
- 参数（带默认）：`--host 192.168.199.155 --user viccom --pass Pa88word --server Matrikon.OPC.Simulation.1 --tag Random.Real4`。
- 阶段 A（想法1，**含 null 对比**）：先 `OpcDaClient::new`（null 凭据，ncpepc）→ 预期 `0x80070005`/权限不足；再 `with_credentials(viccom)` → `list_servers` 含目标、`read Random.Real4` 成功。
- 阶段 B（想法2）：`FusionReader` 跑 ~30s，打印每条数据来源与模式切换。

## 验证标准（成功定义）

1. `cargo build --workspace` 通过；`make verify`（fmt/clippy/test/compat）通过。
2. example 阶段 A：null 凭据连远程 → Access Denied 或权限不足（证明凭据必要性）。
3. example 阶段 A：`viccom/Pa88word` → `list_servers` 含 `Matrikon.OPC.Simulation.1`，`read Random.Real4` 返回值。
4. example 阶段 B：订阅收到推送，或 fallback 同步轮询有数据；输出正确标注来源。
5. 现有 e2e（19 本地 + 4 远程）不破坏。

## 实现顺序

1. `typedefs.rs` Bridge 修复 + 单测（验证不再悬垂：构造带凭据 ServerInfo，try_to_native 后 CoCreateInstanceEx 不崩，或断言指针指向 bridge 内存）。
2. `client.rs` `create_server2`/`get_servers` 接通凭据。
3. `typedefs.rs`/`lib.rs` `AuthCredentials` 类型 + pub 导出。
4. `connector.rs` `ComConnector` credentials + `with_credentials`。
5. `helpers.rs` `connect_server` 凭据参数。
6. `opc_da.rs` `OpcDaClient::with_credentials`。
7. `examples/verify_dcom_auth.rs` + `FusionReader`。
8. `make verify`。
9. 真实环境验证（跑 example，记录阶段 A/B 结果）。

## 风险

- **Bridge 修复的 unsafe 正确性**（三层生命周期）—— 配 `// SAFETY:` 注释 + 单测；32/64 位均验。
- **远程 Matrikon DCOM 配置是否就绪** —— 只有跑了才知道；订阅回调可能不通（由 fallback 兜底，验证程序如实观测）。
- **`get_servers` 签名改动**影响 `ClientTrait` —— 调用点少（`ComConnector::enumerate_servers`），可控。
- **密码安全** —— `AuthCredentials::Debug` 屏蔽密码；`tracing` 不记密码。

## 验证结果（2026-07-31，host 192.168.199.155，viccom/Pa88word）

真机跑 `examples/verify_dcom_auth.rs` 全流程通过：

- ✅ **想法1（DCOM 凭据）完全验证**：viccom 凭据 `list_servers` + `read Random.Real4` 成功，读到真实值（如 `33.51` Good）。`COAUTHIDENTITY` 链路端到端打通。
- ✅ **想法2（融合读取）完全验证**：远程订阅因反向回调不通（`0x800706BA`，client 端未配入站 DCOM）→ 8s 超时 → **fallback 同步 read 成功兜底**，连续读到数据流（`19991.98 / 19025.23 / 8140.13 / 9759.00 / 26469.93 / 125.03`）。这正是想法2 的核心价值——回调不通时同步兜底保数据。
- 🔧 **验证暴露并修复的实现 bug**（之前因 `pAuthInfo:null` 死代码从未触发）：
  - `COAUTHIDENTITY.Flags = 0` → `SEC_WINNT_AUTH_IDENTITY_UNICODE(2)`（否则 `0x80070057`）。
  - `COAUTHINFO.pwszServerPrincName` 用 `RPC_C_AUTHN_WINNT` 时空串指针 → `NULL`（MSDN 要求）。
- 🔧 **想法2 的工程要点**：订阅与兜底读取必须用各自独立的 `OpcDaClient`（独立 COM worker），否则订阅失败/超时会阻塞兜底读取（worker 单线程）；且 subscribe 须限 timeout（反向回调 RPC 超时长达数十秒）。

**未做（范围外）**：sink `CoSetProxyBlanket` / 进程级 `CoInitializeSecurity`。远程订阅回调能否成功仍取决于 client 端入站 DCOM 配置（`DCOM_GUIDE.md` §6.3）——未配时由 fallback 兜底。

质量门：opc-da-client `fmt`/`clippy --all-targets`/`lib(75)`/`doc(10)` 全过；workspace 编译通过。
