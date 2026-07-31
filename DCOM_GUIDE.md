# OPC DA 数据访问与 DCOM 跨主机通讯详解

> 面向 `opc-cli` / `opc-da-client` 的使用者与现场调试人员。
>
> 目标：讲清 OPC DA 三种数据访问机制（**同步 I/O / 异步 I/O / 订阅**）在 DCOM 下
> 的**通讯方向**与**认证差异**，给出本机 vs 远程的配置与排障指南。
>
> 配套真相源：
> - 协议行为契约 → [`opc-da-client/spec.md`](./opc-da-client/spec.md)
> - 库技术设计 → [`opc-da-client/architecture.md`](./opc-da-client/architecture.md)
> - 高层导航图 → [`ARCHITECTURE_DIAGRAM.md`](./ARCHITECTURE_DIAGRAM.md)

---

## 0. 速查表（TL;DR）

| 机制 | COM 接口 | 调用方向 | server→client 回调？ | client 端要配 DCOM？ | 远程典型坑 |
|:---|:---|:---|:---|:---|:---|
| **同步 I/O** | `IOPCSyncIO` / `IOPCSyncIO2` | client → server（单向） | ❌ 不需要 | ❌ 基本不用 | server 端权限/防火墙 |
| **异步 I/O** | `IOPCAsyncIO2/3` + `IOPCDataCallback` | client → server，server 回调 client | ✅ 需要 | ✅ 必须 | 回调连不回 client |
| **订阅** | `IConnectionPoint::Advise` + `IOPCDataCallback` | client → server，server 持续回调 client | ✅ 需要 | ✅ 必须 | 同上，且生命周期更长 |

**一句话结论**：远程读/写（同步 I/O）只需配 **server 端** DCOM；远程订阅/异步 I/O 还要配 **client 端** DCOM（让 client 能作为 DCOM 服务端接收反向回调）。本机（localhost）下三者基本零配置。

---

## 1. 前置概念：COM / DCOM / OPC DA 的分层

- **COM**：Windows 组件对象模型。同一进程、跨进程（本地）、跨机器都用同一套"接口指针 + 方法调用"模型。
- **DCOM**：COM 的网络扩展——**跨机器的 COM**。底层走 RPC（TCP），用 endpoint mapper（TCP 135）协商动态端口。
- **OPC DA**：建立在 DCOM 之上的工业数据访问规范（OPC 基金会）。OPC Server 是一个 COM 服务器，OPC Client 通过 COM 接口指针调用它。

**关键推论**：OPC DA 的所有"远程"行为，本质都是 DCOM。理解 DCOM 的认证与回调，才能理解 OPC DA 远程为什么这样配、这样报错。

---

## 2. OPC DA 的三种数据访问机制

### 2.1 Synchronous I/O（同步 I/O）

**接口**：`IOPCSyncIO`（DA 1.0）、`IOPCSyncIO2`（DA 3.0 增强：`ReadMaxAge` / `WriteVQT`）。

**语义**：客户端发起 `Read` / `Write`，**阻塞**在该次 COM 调用中，直到服务器把所有请求 item 的值/质量/时间戳（或逐项错误码）准备好并一次性返回。

```
Client ──Read(handles)──> Server
Client <──(values+errors)── Server   // 同一个调用栈返回，无后续回调
```

要点：
- 读时有 `source` 参数：`OPC_DS_CACHE`（读内存缓存，快）或 `OPC_DS_DEVICE`（读物理设备，准但慢）。
- **应用层是纯单向调用**：client → server 一发一收，server 不反向调用 client。
- 这是本项目 `read_tag_values` / `write_tag_value` 走的路径（见 §5.1）。

### 2.2 Asynchronous I/O（异步 I/O）

**接口**：`IOPCAsyncIO`（DA 1.0，基于 `IAdviseSink`，已废弃）/ `IOPCAsyncIO2`（DA 2.0）/ `IOPCAsyncIO3`（DA 3.0）。

**语义**：客户端调 `Read` / `Write`，服务器**立刻返回一个 `TransactionID`**（外加 `CancelID`），调用本身不阻塞。真正的结果稍后由服务器**反向回调**客户端注册的 `IOPCDataCallback` sink：

| 回调方法 | 触发时机 |
|:---|:---|
| `OnReadComplete` | 一次异步 Read 完成 |
| `OnWriteComplete` | 一次异步 Write 完成 |
| `OnCancelComplete` | `Cancel2` 取消成功 |
| `OnDataChange` | 见 §2.3 订阅 |

```
Client ──AsyncRead(handles)──> Server        // 立即返回 TransactionID
Client <──OnReadComplete(values)── Server     // 稍后反向回调（新的一次 COM 调用，方向反转）
```

client 必须用 TransactionID 把"我发的请求"和"收到的回调"配对。

### 2.3 Subscription（订阅）

**接口**：`IConnectionPointContainer` → `IConnectionPoint`（找 `IID_IOPCDataCallback`）→ `Advise(clientSink)`。

**语义**：client 通过 `Advise` 把自己实现的 `IOPCDataCallback` sink 注册给 server。之后 server **持续**在以下时机反向调用 client 的 `OnDataChange`：
- 数据变化超过 **deadband**（变化死区，百分比）；
- `update_rate` 周期到（周期刷新，`Refresh2` / `RefreshMaxAge`）；
- **keep-alive** 到期且无变化时发心跳（`IOPCGroupStateMgt2::SetKeepAlive`，本项目有 `set_keep_alive`）。

```
Client ──Advise(IOPCDataCallback)──> Server
Client <──OnDataChange(...)── Server           // 持续推送，直到 Unadvise
```

### 2.4 三者关系（关键澄清）

⚠️ **常见误区**：很多人以为"订阅是建立在异步 I/O 之上的"。**不是。**

准确表述：**异步 I/O 和订阅是并列的两种机制，它们共享同一套 server→client 回调基础设施**（`IOPCDataCallback` sink + `IConnectionPoint::Advise`），区别只在**触发条件**和**生命周期**：

| | 触发 | 回调方法 | 生命周期 |
|:---|:---|:---|:---|
| 异步 I/O | client 主动发一次 Read/Write | `OnReadComplete` / `OnWriteComplete` | 一次性 |
| 订阅 | 数据变化 / 周期 / keep-alive | `OnDataChange` | 持续，直到 `Unadvise` |

在 **DCOM 通讯与认证层面，两者完全等价**：都走反向回调、都受同样的 impersonation / 防火墙 / 权限约束。订阅只是触发更频繁、活得更久。因此本文后续把"异步 I/O 与订阅"统称**回调类机制**，一起讨论。

---

## 3. DCOM 跨主机通讯与安全模型

### 3.1 一次远程 COM 调用的网络链路

client 用 `CoCreateInstanceEx`（带 `COSERVERINFO`）激活远程对象：

```
Client
  │ ① 连 server 机的 TCP 135 (RPC Endpoint Mapper)
  │ ② EPM 返回该 COM 对象的动态端口（高位，通常 49152–65535）
  │ ③ 在动态端口上建立 RPC 会话，完成认证握手
  │ ④ 在该会话上调 IOPCSyncIO::Read 等方法
  ▼
Server
```

防火墙必须放行 **135 + 动态高端口范围**（或把 RPC 限制到固定端口范围，见 §6.4）。

### 3.2 认证模型：凭据是谁的

⚠️ **常见误区**："client 携带 server 的认证（用户名+密码）"。

**正解**：凭据是 **client 自己的**，由 **server 来验证**。`COSERVERINFO.pAuthInfo` 指向 `COAUTHINFO`，其 `pAuthIdentityData` 指向 `COAUTHIDENTITY`。MSDN 原文：`COAUTHIDENTITY` *"establishes a nondefault **client identity**… If this parameter is NULL, the **actual identity of the client is used**."*

三种取法：
- `pAuthInfo = NULL` → 用 client **当前登录身份**（Snego 自动协商认证协议）。**本项目远程分支就用的这种**（见 §5.4）。
- `pAuthInfo` 指向自定义 `COAUTHIDENTITY` → 用 client 指定的**替代身份**（特定用户/域/密码）连 server，server 校验。
- `pAuthInfo` 指向 `RPC_C_AUTHN_LEVEL_NONE` → 不认证（仅内网可信场景）。

无论哪种，**身份属于 client、验证发生在 server**。

### 3.3 三个独立的安全旋钮（最易混淆，务必分清）

DCOM 安全由三个**相互独立**的维度控制。把它们混为一谈是现场排障的头号错误。

#### 旋钮一：Authentication Level（认证级别）—— client→server 的数据保护

MSDN `Authentication Level Constants`，7 级递进（每级含前级保护）：

| 值 | 常量 | 含义 |
|:---|:---|:---|
| 1 | `RPC_C_AUTHN_LEVEL_NONE` | 不认证 |
| 2 | `RPC_C_AUTHN_LEVEL_CONNECT` | 仅连接时认证（OPC 常用默认） |
| 3 | `RPC_C_AUTHN_LEVEL_CALL` | 每次 RPC 调用认证 |
| 4 | `RPC_C_AUTHN_LEVEL_PKT` | 校验数据来自预期 client |
| 5 | `RPC_C_AUTHN_LEVEL_PKT_INTEGRITY` | 校验数据未被篡改 |
| 6 | `RPC_C_AUTHN_LEVEL_PKT_PRIVACY` | 认证 + **加密**（最高） |

**方向**：保护 client↔server 之间传输的数据；client 与 server 协商取**较高一方的下限**。与"谁能调谁"无关。

#### 旋钮二：Impersonation Level（模拟级别）—— client 授权 server 代表自己行事的程度

MSDN `Impersonation Level Constants`，**由 client 设定**，决定 server 拿到 client 身份后能干什么：

| 值 | 常数 | 含义（MSDN 原文要点） |
|:---|:---|:---|
| 1 | `RPC_C_IMP_LEVEL_ANONYMOUS` | client 对 server 匿名 |
| 2 | `RPC_C_IMP_LEVEL_IDENTIFY` | server 只能查 client 身份（做 ACL 检查），不能以 client 身份访问对象 |
| 3 | `RPC_C_IMP_LEVEL_IMPERSONATE` | server 能以 client 身份访问**本地**资源；**impersonation token 只能跨 1 个机器边界** |
| 4 | `RPC_C_IMP_LEVEL_DELEGATE` | server 能以 client 身份访问**本地+远程**资源；**token 可跨任意个机器边界** |

🔑 **这是回调类机制的核心**：client→server 已跨了 1 个机器边界；若 server 还要**反向回调 client**（第 2 个边界），`IMPERSONATE(3)` **不够**，必须 `DELEGATE(4)`（且 Active Directory 里要"信任此计算机进行委派"+ Kerberos）。MSDN 还强调 `COAUTHINFO.dwImpersonationLevel` *"must be RPC_C_IMP_LEVEL_IMPERSONATE or above"*。

#### 旋钮三：Launch / Access Permissions（启动/访问权限）—— 机器 ACL 层面允许谁

通过 `dcomcnfg` 或注册表 `AppID` 配置，是**操作系统级 ACL**，与认证级别/模拟级别正交：
- **Launch and Activation Permissions**：允许谁**启动/激活**这个 COM 服务器（本地/远程）。
- **Access Permissions**：允许谁**访问**已运行的 COM 服务器（本地/远程）。

server 端配错 → `0x80070005 Access Denied`。回调时 client 端也要配这层（因为 client 临时成了 DCOM 服务端）。

### 3.4 反向回调（server→client）的双向认证

回调类机制（异步 I/O / 订阅）要求 server 反向调用 client 的 `IOPCDataCallback` sink。**此时 client 机器临时扮演 DCOM 服务端角色**（"the client machine acts as a DCOM server for callbacks"）。server 用什么身份连回 client，有三种情况：

| 情况 | server 回调身份 | client 端要求 | 适用 |
|:---|:---|:---|:---|
| ① server 模拟 client | client 自己的身份 | impersonation 须达 **DELEGATE**（跨第 2 边界）+ AD 委派 + Kerberos | 安全要求高 |
| ② sink 认证设为 NONE | 不认证 | client 端放行入站 DCOM 即可，无需委派 | **OPC 现场最常见**（内网可信） |
| ③ server 用自身进程身份 | server 的 service account | client 端存在并允许该 account | server 以服务账号运行时 |

情况②通过 `CoSetProxyBlanket` 把 sink 的认证级别设为 `RPC_C_AUTHN_LEVEL_NONE`，**绕开 delegation 难题**——这是工业现场最常用的工程妥协。代价是回调链路无认证保护。

### 3.5 Kerberos vs NTLM

跨机器认证默认尝试 **Kerberos**，失败则 fallback **NTLM**：

| | Kerberos | NTLM |
|:---|:---|:---|
| 依赖 | AD、SPN、双向 DNS 正反解、**时钟同步 < 5 min** | 仅凭据质询 |
| Delegation | 支持（需 AD 配"信任委派"） | 委派支持弱 |
| 安全 | 强 | 弱 |

跨机回调（情况①）**必须 Kerberos + Delegate**；NTLM 无法满足。时钟偏差 >5 分钟是 Kerberos 失败的头号原因。

### 3.6 本机 vs 远程：分水岭

| | 本机 (localhost) | 远程 |
|:---|:---|:---|
| 回调是否跨网络 | 否（loopback） | 是 |
| Impersonation 要求 | `IMPERSONATE` 够用（不跨边界） | 回调需 `DELEGATE` 或 sink NONE |
| 防火墙 | 不涉及 | 必须放行 135 + 动态端口 |
| client 端 DCOM 配置 | 基本不用 | 回调类机制**必须**配 |
| 典型表现 | sync/async/subscription 全工作 | 同步 I/O 通常 OK，回调类机制易报 `0x800706BA` |

🔑 这正是本项目"本机 e2e 全过、远程 read/write 过、远程 subscribe 报 `0x800706BA`"的根因。

---

## 4. 常见理解误区（修正清单）

| # | 误区 | 正解 |
|:---|:---|:---|
| 1 | "client 携带 server 的认证（用户名+密码）" | 凭据是 **client 自己的**，server 验证。`COAUTHIDENTITY` 是"client 的替代身份"。 |
| 2 | "回调时 client 端要有 server 调用的用户认证"（一概而论） | 看身份来源：模拟 client（需 DELEGATE）/ sink NONE（不认证）/ server 自身身份（才需 client 端有该账户）。 |
| 3 | "订阅建立在异步 I/O 之上" | 两者**并列**，共享回调层；触发条件不同（一次性 vs 持续推送）。 |
| 4 | "认证级别 = 模拟级别 = 权限" | 三个**独立**旋钮：认证级别（数据保护）、模拟级别（client 授权 server 代表自己）、权限（ACL 放行）。 |
| 5 | "同步 I/O 也要配 client 端 DCOM" | 同步 I/O 单向调用，**不需要**。回调类机制才需要。 |
| 6 | "本机能用远程就能用" | 本机不跨网络/不受 impersonation 边界限制；远程是另一套问题。 |

---

## 5. 本项目（opc-cli / opc-da-client）实现现状

> 本节经源码核对（行号基于当前 `main` 分支）。

### 5.1 读/写：全部走同步 I/O

公开 API 是 `async fn`，底层 COM 是**同步 I/O**：

| provider 方法 | 底层 COM 接口 |
|:---|:---|
| `read_tag_values` | `IOPCSyncIO::Read`（`OPC_DS_CACHE`/`DEVICE`）— `opc_da/client/traits/sync_io.rs:28` |
| `write_tag_value` / `write_tag_values` | `IOPCSyncIO::Write` — `sync_io.rs:71` |
| `read_tag_values_max_age` | `IOPCSyncIO2::ReadMaxAge`（DA 3.0） |
| `write_tag_value_vqt` | `IOPCSyncIO2::WriteVQT`（DA 3.0） |

调用链：`provider.rs` async fn → `backend/opc_da.rs:135` `worker.send_request(ComRequest::*)` → `com_worker.rs` 专用 COM 线程阻塞调用 → `oneshot` 回执。

**为什么标 `async` 却不卡 UI**：阻塞在 `ComWorker` 专用 COM 线程，TUI 侧 await 的是 `oneshot` 回执。**与 OPC DA 的 AsyncIO 无关**，别被 `async fn` 字样误导。

### 5.2 订阅：IOPCDataCallback（回调类机制）

```
provider.rs subscribe → backend/opc_da.rs:253 ComRequest::Subscribe
  → com_worker.rs handle_subscribe → create_group_and_advise → build_and_advise_data_callback (com_worker.rs:1983)
  → connector.rs:814 ComGroup::advise_data_callback → IConnectionPoint::Advise(sink)
  → server 反向调用 sink::OnDataChange → mpsc → SubscriptionHandle.rx (provider.rs:147)
```

sink 实现是 `DataCallbackSink`（`subscription.rs:36`），用 `#[implement(IOPCDataCallback)]` 生成 COM 可回调对象；`OnDataChange`/`OnReadComplete`/`OnWriteComplete`/`OnCancelComplete` 均委托 `forward_data_change`（`subscription.rs:122`），用 `tx.try_send` 非阻塞转发。

⚠️ **关键代码事实**：sink **没有调用 `CoSetProxyBlanket`**，整个进程也**没有 `CoInitializeSecurity`**（见 §5.3）。因此 server→client 反向回调的认证级别继承进程/注册表默认。这是远程订阅易报 `0x800706BA` 的**代码侧根因之一**。

### 5.3 COM 初始化与线程亲和性

- `com_guard.rs:48` `ComGuard::new()` 只做 `CoInitializeEx(None, COINIT_MULTITHREADED)`（MTA），用 `PhantomData<*mut ()>` 标记 `!Send` 强制线程绑定。
- `com_worker.rs:543` 专用 COM 线程入口调用 `ComGuard::new()`，**独占所有 COM 指针**。
- ⚠️ **全工作区无 `CoInitializeSecurity` 调用** → 进程采用 COM 默认安全设置（注册表 `LegacySecurityLevel` / `MachineAccessPermissions`）。这正是 §5.2 sink 认证级别"继承默认"的原因。
- 连接池按 ProgID 索引；`dispatch_with_retry`（`com_worker.rs:983`）在检测到 RPC 连接错误时驱逐失效代理、重连、重试。

### 5.4 DCOM 认证链路：已接通（支持手动指定 user/password）

> **历史**：曾长期"类型齐全但链路断开"——`create_server2`/`get_servers` 硬编码 `pAuthInfo: null`、`AuthInfo` 被丢弃、Bridge `try_to_native()` 悬垂。已于 2026-07-31 接通并修复，设计见 [`docs/superpowers/specs/2026-07-31-dcom-auth-fusion-design.md`](./docs/superpowers/specs/2026-07-31-dcom-auth-fusion-design.md)。

**现状**：远程 DCOM 支持手动指定 `user`/`password`/`domain`。

| 层 | 说明 |
|:---|:---|
| 用户态凭据 `AuthCredentials` | `{ user, password, domain }`，`Debug` 屏蔽密码；`to_auth_info()` 生成 `AuthInfo`（`RPC_C_AUTHN_WINNT` + `CONNECT` 认证级 + `IMPERSONATE` 模拟级 + `SEC_WINNT_AUTH_IDENTITY_UNICODE`） |
| 注入入口 | `OpcDaClient::with_credentials(host, creds)` / `ComConnector::with_credentials(host, creds)` —— client 构造层注入，`OpcProvider` trait 签名不变，所有方法自动受益 |
| FFI Bridge | **已修复悬垂**：`AuthInfoBridge`/`AuthIdentityBridge` 以 `native: Box<COAUTH*>` 作为 owned 字段，三层（`COSERVERINFO`→`COAUTHINFO`→`COAUTHIDENTITY`→wide string）全由 Bridge 持有、堆地址固定，move 后指针仍有效（回归测试 `auth_identity_native_pointers_survive_move`） |
| 远程激活 | `create_server2`/`get_servers` 用 Bridge 构造带凭据 `COSERVERINFO`；**`user` 为空 → `pAuthInfo: null`**（当前登录用户，向后兼容） |

**用法**：

```rust
use opc_da_client::{AuthCredentials, OpcDaClient, OpcProvider};

let client = OpcDaClient::with_credentials("192.168.199.155", AuthCredentials {
    user: "viccom".into(),
    password: "...".into(),
    domain: String::new(), // 本地帐户留空；域帐户填域名
})?;
let vals = client
    .read_tag_values("Matrikon.OPC.Simulation.1", vec!["Random.Real4".into()])
    .await?;
```

**已验证**（2026-07-31，192.168.199.155 / viccom）：带凭据 `list_servers` + `read` 成功，读到真实值；`0x80070057`（凭据编码）/`0x80070533`（帐户禁用）等现场问题由 `friendly_hresult_hint` 翻译。详见 spec 验证结果。

#### 当前用户 token vs 显式凭据（跨工作组 SID）

⚠️ **工作组（非域）跨机器：当前登录用户 token 不可移植，必须显式凭据。**

`OpcDaClient::new(ComConnector::new(host))`（不传 `AuthCredentials`）走 `pAuthInfo: null`，DCOM 用 client 进程的**当前登录用户 token**。但本机帐户的 SID 与远程机同名帐户的 SID 不同（除非同域），远程 launch/access permission 按 SID 比对时对不上 → `0x80070005`。`OPCServerList` 认证宽松会掩盖（`list_servers` 成功），但 DA server 本体严格会暴露。

显式 `AuthCredentials`（`COAUTHIDENTITY`）给出 `user`/`password`，server 用**自己本地**的该帐户认证，SID/密码/permission 全对得上 → 成功。

**实战诊断矩阵**（192.168.199.155 / Matrikon，viccom 与 ncpepc 均启用且 viccom 有 launch 权限）：

| # | 凭据方式 | read 结果 | 说明 |
|:--|:--|:--|:--|
| 1 | ncpepc 当前用户 token（null） | ❌ `0x80070005` | 本机 ncpepc token SID ≠ 远程 ncpepc，launch permission 不匹配 |
| 2 | viccom 显式凭据（正确密码） | ✅ 读到值 | `COAUTHIDENTITY` → 远程本地 viccom 认证 |
| 3 | viccom 显式凭据（错误密码） | ❌ `0x80070005` | 证明代码确实用 viccom 认证（拒错密码） |
| 4 | ncpepc 显式凭据（正确密码） | ✅ 读到值 | 同一 ncpepc 帐户，显式凭据成功、当前 token 失败 → 定位为 token SID 问题 |

**结论**：跨工作组远程 DA server，用 `OpcDaClient::with_credentials`（显式 user/password），不要依赖当前登录用户 token。

`is_remote_host`（`helpers.rs`）：空 host = 本地 `CoCreateInstance`；任何非空 host（含 `localhost`）= DCOM。

### 5.5 订阅健康监控与自愈（生产可用性关键）

专为「远程 DCOM 回调静默死亡」设计的容错：

- `spawn_health_monitor`（`com_worker.rs:383`）：每 1s 扫描，`last_update` 陈旧超过 `max(update_rate×3, 30s)` → 触发重建。
- 重建分级：先轻量 `readvise_existing`（`com_worker.rs:2258`，unadvise 旧 sink + 同 channel 重 advise）；遇连接错误则全量 `reconnect_subscription`（`com_worker.rs:2310`，驱逐死 server、DCOM/SCM 重启、重建 group/items/sink）。
- `is_connection_error`（`com_worker.rs:504`）识别 `0x800706BA`/`0x800706BF`/`0x800706BE`/`0x80080005`；`dispatch_with_retry`（`com_worker.rs:983`）最多 3 次重连 + 指数退避（50/100/200ms）。

含义：即使远程回调因网络/DCOM 抖动静默死亡，订阅也会在约 30s 内自愈。这是远程订阅「可用」而非「完全无坑」的保障（即已完成的 P0-1 订阅续订计划）。

### 5.6 诊断 CLI：`examples/remote_list.rs`

最小化 DCOM 连通性诊断（`examples/remote_list.rs:1`）：取 `argv[1]` 为 host，`list_servers` 枚举并打印 ProgID，失败打印原始 HRESULT + `friendly_com_hint`。**排障第一步**。区分位宽：

```sh
cargo run -p opc-da-client --example remote_list -- 192.168.199.155                        # 64-bit
cargo run -p opc-da-client --target i686-pc-windows-msvc --example remote_list -- <host>    # 32-bit（匹配 Takebishi 等遗留 client）
```

### 5.7 HRESULT 友好提示：`opc_da/errors.rs`

`friendly_hresult_hint`（`errors.rs:74`）是全工作区唯一的 HRESULT 语义字典，经 `format_hresult`（`errors.rs:65`）/`friendly_com_hint`（`errors.rs:98`）暴露；`OpcError::Com` 的 `#[error]` 自动内联，故任何 `?` 传播的 COM 错误都带可读提示。详见 §6.7 错误码表。

---

## 6. 现场调试实操

### 6.1 本机（localhost）：基本零配置

OPC Server 与 client 同机：不跨网络、无防火墙、`IMPERSONATE` 够用。sync/async/subscription 通常开箱即用。若本机都报错，先查 server 是否注册、是否以正确身份运行。

### 6.2 远程读/写（同步 I/O）：只需配 server 端

同步 I/O 单向调用，**只需在 server 机配置**：

1. **server 端 `dcomcnfg`**（`dcomcnfg.exe` → 组件服务 → 计算机 → My Computer → DCOM Config → 选中 OPC Server）：
   - **Security → Launch and Activation Permissions**：加 client 用户（或 `Everyone`/`Distributed COM Users`），勾选 Remote Launch / Remote Activation。
   - **Security → Access Permissions**：同上用户，勾选 Remote Access。
   - **Identity**：建议"This user"指定专用服务账号（生产）或"The interactive user"（调试）。
2. **server 端防火墙**：放行 TCP 135 + 动态 RPC 端口（见 §6.4）。
3. **server 端用户**：client 用的身份（登录身份或 `COAUTHIDENTITY`）在 server 上必须有效且被允许。

client 端**一般无需特殊配置**（出站 DCOM 默认允许）。

### 6.3 远程订阅 / 异步 I/O（回调）：最复杂，还要配 client 端

在 §6.2 基础上，额外配置 **client 端**（因为 client 要作为 DCOM 服务端接收回调）：

1. **client 端防火墙**：放行入站 TCP 135 + 动态 RPC 端口（见 §6.4）。
2. **client 端 DCOM 访问权限**：`dcomcnfg` → My Computer → 属性 → COM Security → Access Permissions → Edit Default：确保回调调用者有 Local+Remote Access。
   - 情况①（模拟 client）：client 身份本身在 client 机当然存在；但需 server 侧 impersonation 达 DELEGATE + AD 委派（见 §6.5）。
   - 情况②（sink NONE）：client 端放行入站即可，无需委派——**优先尝试这种**。
   - 情况③（server service account）：client 端要有该 account 并放行。
   - **本项目现状**（见 §5.2/§5.3）：sink 未显式 `CoSetProxyBlanket`、进程无 `CoInitializeSecurity`，回调认证继承进程/注册表默认。远程订阅需靠 client 端 COM Security 默认放行 + impersonation 协商；若仍不稳定，后续可考虑给 sink 显式设 `RPC_C_AUTHN_LEVEL_NONE`。好在订阅带健康监控自愈（§5.5），回调静默死亡约 30s 自动重建。
3. **client 端 RPC 服务**：`Remote Procedure Call (RPC)` 与 `DCOM Server Process Launcher` 已启动。

### 6.4 Windows 防火墙：135 + 动态端口

DCOM 用 **TCP 135**（Endpoint Mapper）+ **动态高端口**（Server 2008+ 默认 49152–65535）。

```powershell
# 放行 DCOM / COM+ 网络访问（client 与 server 两端，回调场景）
Enable-NetFirewallRule -Name "ComPlusNetworkAccess","DCOM-IN"
netsh advfirewall firewall set rule group="Windows Management Instrumentation (WMI)" new enable=yes
netsh advfirewall firewall set rule group="Remote Administration" new enable=yes
```

若防火墙不能开整个动态范围，可把 RPC 限制到固定端口范围（注册表 `HKLM\Software\Microsoft\Rpc\Internet`：`Ports` = "5000-5100"、`PortsInternetAvailable` = "Y"、`UseInternetPorts` = "Y"）。

### 6.5 Kerberos 配置要点（回调走"模拟 client"时必看）

- **时钟同步**：client 与 server 时钟偏差 **< 5 分钟**，否则 Kerberos 直接失败（头号原因）。
- **DNS 正反解**：双方机器名要能正反向解析。
- **SPN**：server 的服务主体名注册正确（`setspn -L <server>`）。
- **委派**：AD 里把 server 计算机账号设为"信任此计算机进行 delegation"（情况①回调必需）。

不想碰 Kerberos delegation → 走 §3.4 情况②（sink 认证 NONE）规避。

### 6.6 `dcomcnfg` 逐步配置（server 端，最常用）

1. `Win+R` → `dcomcnfg` → 组件服务 → 计算机 → My Computer → DCOM Config。
2. 找到 OPC Server（如 `Matrikon.OPC.Simulation` / `Kepware.KEPServerEX`），右键 → 属性。
   - 未列出 → 先 `opcenum /regserver` 或 server 自带注册工具注册。
3. **General**：Authentication Level = Default 或 Connect。
4. **Security**：
   - Launch and Activation Permissions → Customize → Edit → 加 client 用户 → 勾 Remote Launch + Remote Activation。
   - Access Permissions → Customize → Edit → 加同用户 → 勾 Remote Access。
5. **Identity**：选专用服务账号（生产）或 interactive user（调试）。
6. **Endpoints**：确保 Connection-oriented TCP/IP 启用。
7. 顶层 **My Computer → 属性**：
   - Default Properties：勾"Enable Distributed COM"；Default Impersonation Level = Identify 或 Impersonate。
   - COM Security：默认 Access/Launch 权限里也要有相关用户。

### 6.7 错误码对照与排障决策树

| HRESULT | 含义 | 优先排查 |
|:---|:---|:---|
| `0x80070005` E_ACCESSDENIED | 访问被拒（权限/认证） | server 端 Launch/Access Permissions 是否含 client 身份；身份在 server 是否有效 |
| `0x800706BA` RPC_S_SERVER_UNAVAILABLE | RPC 不可用 | 防火墙（135+动态端口）、RPC 服务未起、**回调连不回 client**（client 端入站/权限） |
| `0x800703E6` | 内存访问无效 | 本项目已修的 32 位 `COAUTHINFO` 悬垂指针；若再现检查凭据内存生命周期 |
| `0x8007000D` / `0x80040154` | 参数/类未注册 | server 未注册、ProgID/CLSID 错 |
| `0x80010108` RPC_E_DISCONNECTED | 对象已断开 | server 重启 / 长时间无心跳，触发连接池驱逐重连 |
| `0x800706F4` | COM 编组（marshalling）错误 | 尝试重启 OPC server |
| `0x80080005` SERVER_EXEC_FAILED | 服务器进程启动失败 | server 端 Identity 账号权限 / server 可执行文件路径 |
| `0x80040112` / `0x80040154` | license 拒绝 / 类未注册 | server 许可证 / server 未注册、ProgID 错 |

> 上述翻译来自 `opc-da-client/src/opc_da/errors.rs:74` 的 `friendly_hresult_hint`（经 `format_hresult` / `friendly_com_hint` 暴露）。

**排障决策树**：

```
远程报错
 ├─ 读/写（同步 I/O）失败
 │    ├─ 0x80070005 → server 端 dcomcnfg 权限 + 身份有效性
 │    └─ 0x800706BA → server 端防火墙(135)/RPC 服务/server 是否运行
 └─ 订阅/异步（回调）失败，但读/写正常
      └─ 0x800706BA → 90% 是回调连不回 client：
           1) client 端防火墙放行入站 135+动态端口
           2) client 端 DCOM Access Permissions 放行调用者
           3) 改用 sink NONE（§3.4 ②）规避 delegation
           4) 或配 Kerberos DELEGATE（§6.5）
```

### 6.8 诊断命令清单

```powershell
dcomcnfg                              # 图形化 DCOM 配置
services.msc                          # 确认 RPC / DCOM Server Process Launcher 已启动
setspn -L <server>                    # 查 server 的 SPN（Kerberos）
klist                                 # 查 Kerberos 票据（含是否拿到 server 票）
w32tm /query /status                  # 时钟同步状态（Kerberos < 5min）
portqry -n <server> -e 135            # 探测 server 端 135 是否可达
rpcping -t ncacn_ip_tcp -s <server>   # RPC 连通性探测
```

本项目自带：`cargo run --example remote_list`（`examples/remote_list.rs`）——现场第一步先用它确认 client→server 的 DCOM 连通与服务器枚举。

---

## 7. 参考文献

**微软 Win32 / COM 一手文档**（已核对）：
- [COAUTHINFO structure](https://learn.microsoft.com/en-us/windows/win32/api/wtypesbase/ns-wtypesbase-coauthinfo)
- [COAUTHIDENTITY structure](https://learn.microsoft.com/en-us/windows/win32/api/wtypesbase/ns-wtypesbase-coauthidentity)
- [Impersonation Level Constants](https://learn.microsoft.com/en-us/windows/win32/com/com-impersonation-level-constants)
- [Authentication Level Constants](https://learn.microsoft.com/en-us/windows/win32/com/com-authentication-level-constants)
- [COSERVERINFO structure](https://learn.microsoft.com/en-us/windows/win32/api/objidl/ns-objidl-coserverinfo)
- [CoInitializeSecurity](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-coinitializesecurity) / [CoSetProxyBlanket](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-cosetproxyblanket)

**本项目内部真相源**：
- [`opc-da-client/spec.md`](./opc-da-client/spec.md) — 行为契约
- [`opc-da-client/architecture.md`](./opc-da-client/architecture.md) — 技术设计
- [`CLAUDE.md`](./CLAUDE.md) — 已知坑（`AuthInfo` 悬垂指针、远程订阅 `0x800706BA`、位宽与 `IOPCServerList` 枚举等）
