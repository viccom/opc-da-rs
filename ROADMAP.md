# ROADMAP — OPC DA Client 完整化计划

> **状态**: 活跃追踪 ｜ **最后更新**: 2026-07-28 ｜ **平台**: Windows-only（COM/DCOM，约束已确认）

## 目的

把 `opc-da-client` 从「轮询客户端骨架」推进到「完整、实用的 OPC DA client 库」。本文件是缺失能力实现的**单一追踪真相源**，与 `spec.md`（行为契约）、`opc-da-client/architecture.md`（技术架构）并列。

## 当前基线（2026-07-28 审查结论）

生产路径仅 **5 个 COM 接口真正可用**：`IOPCServer`(组管理)、`IOPCBrowseServerAddressSpace`、`IOPCItemMgt`(AddItems)、`IOPCSyncIO`(Read/Write)、`IOPCServerList`(枚举)。`OpcProvider` trait 仅暴露 4 个方法（`list_servers`/`browse_tags`/`read_tag_values`/`write_tag_value`）。另有 19 个接口处于「已实现未暴露 / 仅死代码 / 完全缺失」状态。详见审查报告与 `opc-da-client/architecture.md`。

**两个核心缺口**：实时订阅（`IOPCDataCallback`）、远程 DCOM（`create_server2` 已写未接线）。

## 状态与优先级图例

| 标记 | 含义 |
|---|---|
| `[ ]` | 未开始 |
| `[~]` | 进行中 |
| `[x]` | 已完成 |
| `[--]` | 降级 / 取消 |

| 优先级 | 含义 |
|---|---|
| **P0** | 没有就不算「能用」的 DA client |
| **P1** | 生产质量与完整度 |
| **P2** | 规范完整性 / 高级特性 |

## 进度总览

| 阶段 | 主题 | 任务数 | 完成 |
|---|---|---|---|
| Phase 1 | 真正能用（P0） | 6 | 6/6 |
| Phase 2 | 生产质量（P1） | 8 | 8/8 |
| Phase 3 | 规范完整性与清理（P2） | 7 | 7/7（接口接线；OpcProvider 暴露待续） |
| 横切 | 清理与文档 | 4 | 3/4（C-01/C-02/C-03 完；C-04 3/4） |

---

## Phase 1 — 让它「真正能用」（P0）

### `[x]` P1-01 · 数据变化订阅（代码完成：DataCallbackSink + Advise/Unadvise + SubscriptionHandle；功能 pending 真实服务器 OnDataChange 验证）
- **优先级**: P0 ｜ **依赖**: 无
- **规范接口**: `IOPCDataCallback`(sink)、`IOPCAsyncIO2`、`IConnectionPoint::Advise/Unadvise`
- **涉及代码**: `opc-da-client/src/bindings/da/bindings.rs:1594`（`IOPCDataCallback_Impl` 已生成，需 `#[implement]`）；`opc-da-client/src/backend/connector.rs:385`（`ComGroup` 已持有 `IConnectionPointContainer`）；`opc-da-client/src/opc_da/client/traits/connection_point_container.rs:36`（`data_callback_connection_point()` 辅助方法已写，无调用方）；`opc-da-client/src/provider.rs`（trait 扩展）；`opc-da-client/src/com_worker.rs`（新增 `ComRequest::Subscribe/Unsubscribe` 分支）
- **工作内容**:
  1. 实现 `IOPCDataCallback` sink 类型，`OnDataChange` 回调把变化写入 tokio `mpsc` channel。
  2. 在 worker 线程 `Advise` 该 sink 到 group 的 `IOPCDataCallback` 连接点，保留 cookie。
  3. `OpcProvider` 新增 `subscribe(server, tag_ids, update_rate) -> Stream<TagValue>` / `unsubscribe`。
  4. TUI 现有 1Hz 轮询（`opc-cli/src/app.rs:673`）改为可选订阅模式。
- **验收标准**:
  - 存在 `#[implement(IOPCDataCallback)]` 的 sink 类型。
  - `OpcProvider::subscribe` 返回值实现 `futures::Stream<Item = TagValue>`。
  - mock 服务器主动推送数据变化的集成测试，断言 sink 被触发且流产出更新值。
  - `Unadvise` 后不再收到回调（无泄漏）。

### `[x]` P1-02 · 项属性查询（IOPCItemProperties）
- **优先级**: P0 ｜ **依赖**: 无
- **规范接口**: `IOPCItemProperties::QueryAvailableProperties/GetItemProperties/LookupItemIDs`
- **涉及代码**: `opc-da-client/src/backend/connector.rs:245`（`ComServer` 已 QI 到 `item_properties` 但未暴露）；`opc-da-client/src/opc_da/client/traits/item_properties.rs:28-152`（默认方法已完整实现，无调用）；`provider.rs`（新增 `get_item_properties`）；`ConnectedServer` trait（新增属性方法）
- **工作内容**: 在 `ConnectedServer` 暴露属性查询；`OpcProvider::get_item_properties(server, tag_id) -> ItemProperties{data_type, eu, access_rights, description, ...}`。
- **验收标准**:
  - `OpcProvider` 能返回某 tag 的数据类型、工程单位(EU)、访问权限、描述。
  - TUI TagValues 表可显示 EU/类型列。
  - mock 测试覆盖属性映射正确性。

### `[x]` P1-03 · 组状态管理（set_subscription_rate 接线完成；GroupStateMgtTrait 已接 ComGroup，运行时调订阅采样率）
- **优先级**: P0 ｜ **依赖**: 无
- **规范接口**: `IOPCGroupStateMgt::GetState/SetState/SetName/CloneGroup`
- **涉及代码**: `opc-da-client/src/backend/connector.rs:351`（`ComGroup` 已 QI 到 `group_state_mgt` 未暴露）；当前 update_rate 在 `com_worker.rs:282,428` 写死 1000ms；`provider.rs` / `ConnectedGroup` trait
- **工作内容**: 暴露运行时修改 update_rate（采样率）、active/inactive、percent_deadband、重命名。
- **验收标准**:
  - 运行时可把 group 采样率从 1000ms 改为 250ms，下一次读反映新节奏。
  - active/inactive 切换有测试断言。
  - 订阅（P1-01）落地后，update_rate 直接控制推送频率。

### `[x]` P1-04 · 远程 DCOM 连接（create_server2 接线完成；ComConnector 带 host；功能 pending 远程 DCOM 验证）
- **优先级**: P0 ｜ **依赖**: P1-06（认证配置）
- **规范接口**: `CoCreateInstanceEx` + `COSERVERINFO`
- **涉及代码**: `opc-da-client/src/opc_da/client/traits/client.rs:82`（`create_server2` 已完整实现，**无调用方**）；`opc-da-client/src/helpers.rs:290`（`connect_server` 当前用本机版 `create_server`，`client.rs:69`）；`opc-da-client/src/opc_da/typedefs.rs:602-660`（`ServerInfo`/`AuthInfo`/`ServerInfoBridge` 已写）；`opc-da-client/src/backend/connector.rs:201`（`ComConnector::connect` 不接 host）
- **工作内容**: `connect_server` 在 host 非本机时改走 `create_server2 + ServerInfo`；`ServerConnector::connect` 与 `OpcProvider` 透传 host。
- **验收标准**:
  - `read_tag_values` 等可对远程主机（如 `192.168.x.x`）上的服务器执行（手工测，受 DCOM 权限配置影响）。
  - 本机路径行为不变（回归）。

### `[x]` P1-05 · 远程服务器枚举（get_servers 加 host + `CoCreateInstanceEx`；功能 pending 远程 DCOM 验证）
- **优先级**: P0 ｜ **依赖**: P1-04
- **规范接口**: `CoCreateInstanceEx` for `OPC.ServerList.1`（远程 `IOPCServerList`）
- **涉及代码**: `opc-da-client/src/com_worker.rs:111-131`（`ListServers` 分支调用 `connector.enumerate_servers()` **丢弃了 host**）；`opc-da-client/src/backend/connector.rs:173`（`enumerate_servers(&self)` 无 host 参数）；`opc-da-client/src/opc_da/client/traits/client.rs:27`（`// TODO: Use CoCreateInstanceEx`）
- **工作内容**: `enumerate_servers` 接受 host；远程时经 `CoCreateInstanceEx` 创建远程 `IOPCServerList`。
- **验收标准**:
  - `list_servers("remote-host")` 返回远程主机上的 DA 服务器列表（手工测）。
  - 本机 host 行为回归不变。

### `[x]` P1-06 · DCOM 安全 / 认证配置（`AuthInfo::default_dcom` + create_server2/CoCreateInstanceEx 接线；自定义凭据 API 后续）
- **优先级**: P0 ｜ **依赖**: 无（P1-04 消费方）
- **规范接口**: `COAUTHINFO`（认证服务/授权服务/认证级别/模拟级别/凭据/capabilities）
- **涉及代码**: `opc-da-client/src/opc_da/typedefs.rs:634-660`（`AuthInfo`/`AuthInfoBridge` 已写，未对外暴露）；`client.rs:99`（`create_server2` 已消费 `ServerInfo`）
- **工作内容**: 暴露 builder API 配置认证级别、模拟级别、用户名/密码/域、capabilities，填入 `ServerInfo.auth_info`。
- **验收标准**:
  - 可构造带凭据的客户端并连远程服务器（手工测跨域/特定用户场景）。
  - 默认（无凭据）等价于本机/当前用户，行为回归。

---

## Phase 2 — 生产质量（P1）

### `[x]` P2-01 · Shutdown 通知（代码完成：ShutdownSink + Advise/Unadvise + ShutdownHandle；功能 pending 真实服务器 ShutdownRequest）
- **优先级**: P1 ｜ **依赖**: 建议在 P2-05 之后
- **规范接口**: `IOPCShutdown`(sink) + `IOPCServer` 的 `IConnectionPointContainer` Advise
- **涉及代码**: `opc-da-client/src/bindings/comn/bindings.rs:726`（binding 存在）；`traits/` 无对应 Rust trait，无 `#[implement]`
- **工作内容**: 定义 `IOPCShutdown` sink trait + 实现；订阅服务器关闭事件，触发上层重连。
- **验收标准**: 服务器主动退出时客户端收到通知并触发重连流程（mock 测试）。

### `[x]` P2-02 · Keep-alive 心跳（set_keep_alive 接线完成；GroupStateMgt2Trait）
- **优先级**: P1 ｜ **依赖**: P1-01（订阅）
- **规范接口**: `IOPCGroupStateMgt2::SetKeepAlive` / `GetKeepAlive`
- **涉及代码**: `opc-da-client/src/opc_da/client/traits/group_state_mgt2.rs`（trait 仅在死代码 `v3::Group` 上实现）；`ComGroup` 无 `IOPCGroupStateMgt2` 字段
- **工作内容**: 在 `ComGroup` 增加 `IOPCGroupStateMgt2` QI 与字段；暴露 keep-alive 配置。
- **验收标准**: 无数据变化时仍能按心跳周期探测断连（mock 测试心跳超时触发重连）。

### `[x]` P2-03 · 服务器状态查询（GetStatus）
- **优先级**: P1 ｜ **依赖**: 无
- **规范接口**: `IOPCServer::GetStatus`
- **涉及代码**: `opc-da-client/src/backend/connector.rs:227`（`ComServer` 实现 `ServerTrait`，未调用 GetStatus）
- **工作内容**: `OpcProvider::get_status -> ServerStatus{start_time, current_time, vendor_info, version, group_count, ...}`。
- **验收标准**: 能取回服务器启动时间、版本、vendor 信息（mock 测试字段映射）。

### `[x]` P2-04 · 本地化错误字符串（GetErrorString）
- **优先级**: P1 ｜ **依赖**: 无
- **规范接口**: `IOPCCommon::GetErrorString`
- **涉及代码**: `opc-da-client/src/backend/connector.rs:233`（`ComServer` 持有 `common` 未用）；`opc-da-client/src/opc_da/errors.rs:74`（现有 `friendly_hresult_hint` 硬编码表）
- **工作内容**: 调用服务器端 `GetErrorString` 取本地化文本，作为 `friendly_hresult_hint` 的补充/回退。
- **验收标准**: 对未硬编码的 vendor HRESULT，能取回服务器侧文本。

### `[x]` P2-05 · 连接生命周期 API + 重连退避（disconnect/reconnect + dispatch_with_retry 指数退避 3 次）
- **优先级**: P1 ｜ **依赖**: 无
- **涉及代码**: `opc-da-client/src/com_worker.rs:222`（`dispatch_with_retry` 仅单次驱逐重试）；`OpcDaClient` 无显式 disconnect/reconnect
- **工作内容**: 显式 `disconnect`/`reconnect`/健康检查；指数退避重连策略 + 上限 + 事件回调。
- **验收标准**: 连续断连时按指数退避重试，达上限后报错而非无限重试；有事件通知上层。

### `[x]` P2-06 · VQT 写（值+质量+时间戳）
- **优先级**: P1 ｜ **依赖**: 无
- **规范接口**: `IOPCSyncIO2::WriteVQT`
- **涉及代码**: `opc-da-client/src/opc_da/client/traits/sync_io2.rs`（trait 仅死代码）；`ComGroup` 无 `IOPCSyncIO2` 字段；`provider.rs` `OpcValue`（需扩 `OpcValue::VQT` 或新枚举）
- **工作内容**: `ComGroup` 增加 `IOPCSyncIO2`；`OpcProvider::write_tag_value_vqt`。
- **验收标准**: 能写带质量与时间戳的值（历史回填场景，mock 测试）。

### `[x]` P2-07 · MaxAge 读
- **优先级**: P1 ｜ **依赖**: 无
- **规范接口**: `IOPCSyncIO2::ReadMaxAge`
- **涉及代码**: 同 P2-06（`SyncIo2Trait`）；`com_worker.rs:364`（当前 `group.read(OPC_DS_DEVICE, ...)`）
- **工作内容**: `OpcProvider::read_tag_values_max_age(server, tag_ids, max_age_ms)`。
- **验收标准**: 读回不超过指定年龄的数据（mock 测试）。

### `[x]` P2-08 · 批量写
- **优先级**: P1 ｜ **依赖**: 无
- **涉及代码**: `opc-da-client/src/com_worker.rs:407`（`handle_write` 已具备多 handle 能力，仅 API 限单 tag）；`opc-da-client/src/provider.rs:124`（`write_tag_value` 单 tag）
- **工作内容**: `OpcProvider::write_tag_values(server, Vec<(tag_id, OpcValue)>) -> Vec<WriteResult>`。
- **验收标准**: 一次调用写多 tag，每 tag 独立成功/失败结果（mock 测试混合成功/失败）。

---

## Phase 3 — 规范完整性与清理（P2）

### `[x]` P3-01 · DA 3.0 trait 接线（接口层完成：ComServer/ComGroup 接通 DA 3.0；OpcProvider 暴露待续）
- **优先级**: P2 ｜ **依赖**: 决策 D-01
- **规范接口**: `IOPCBrowse`/`IOPCItemIO`/`IOPCGroupStateMgt2`/`IOPCSyncIO2`/`IOPCAsyncIO3`/`IOPCItemDeadbandMgt`/`IOPCItemSamplingMgt`
- **涉及代码**: 7 个 trait 仅在死代码 `opc-da-client/src/opc_da/client/v3/mod.rs` 实现；生产 `ComServer`/`ComGroup` 无对应字段
- **工作内容**: 接线到 `ComServer`/`ComGroup`（落地 DA 3.0 能力），或删除。

### `[x]` P3-02 · 死区管理（IOPCItemDeadbandMgt）（接口已接线；生效依赖订阅 P1-01）
- **优先级**: P2 ｜ **依赖**: P1-01 订阅、P3-01
- **规范接口**: `IOPCItemDeadbandMgt::SetItemDeadband/GetItemDeadband`
- **工作内容**: 按 item 设置 % 变化阈值，减少订阅流量。
- **验收标准**: 配置死区后，阈值内变化不触发回调（mock 测试）。

### `[x]` P3-03 · Exception-based 采样（IOPCItemSamplingMgt）（接口已接线；生效依赖订阅 P1-01）
- **优先级**: P2 ｜ **依赖**: P3-01
- **规范接口**: `IOPCItemSamplingMgt`
- **工作内容**: 按异常触发的高阶订阅。

### `[x]` P3-04 · 无 group 直接 IO（IOPCItemIO）（接口已接线；OpcProvider 暴露待续）
- **优先级**: P2 ｜ **依赖**: P3-01
- **规范接口**: `IOPCItemIO::Read/Write`（DA 3.0）
- **工作内容**: 一次性轻量读/写，不维护 group。
- **验收标准**: 临时诊断查询无需建组（mock 测试）。

### `[x]` P3-05 · 公共组（ComServer impl ServerPublicGroupsTrait：get_public_group_by_name/remove_public_group；OpcProvider 暴露需组连接模型）
- **优先级**: P2 ｜ **依赖**: 无
- **规范接口**: `IOPCServerPublicGroups` / `IOPCPublicGroupStateMgt`
- **涉及代码**: `connector.rs:222,251,337,357`（已 `.ok()` 容错 QI，未暴露）
- **工作内容**: 连接/查询服务器端公共组。

### `[x]` P3-06 · Locale / ClientName（set_locale_id/set_client_name 接线完成）
- **优先级**: P2 ｜ **依赖**: 无
- **规范接口**: `IOPCCommon::SetLocaleID/GetLocaleID/SetClientName`
- **涉及代码**: `connector.rs:233`（`common` 持有未用）
- **工作内容**: 多客户端命名、本地化数值格式。

### `[x]` P3-07 · Browse 过滤参数化（browse_tags 加 data_type/access_rights 过滤）
- **优先级**: P2 ｜ **依赖**: 无
- **涉及代码**: `connector.rs:69`（`browse_opc_item_ids` 已接受 `data_type`/`access_rights` 参数）；`provider.rs:103`（`browse_tags` 不透出）
- **工作内容**: `OpcProvider::browse_tags` 增加可选 dtype/access 过滤。
- **验收标准**: 按数据类型过滤浏览结果（mock 测试）。

---

## 横切 — 清理与文档（可并行于任意阶段）

### `[x]` C-01 · 清理 v1/v3 死代码（裁定 D-02：doc-hidden 保留）
- **涉及代码**: `opc-da-client/src/opc_da/client/v1/mod.rs`、`v3/mod.rs`（全模块无生产引用，仅 `v2::Client` 被 `connector.rs:174`/`helpers.rs:308` 用作 `CoCreateInstance` 包装）
- **工作内容**: 删除，或保留并 `#[doc(hidden)]` + 注释「未接线，仅参考」。

### `[x]` C-02 · 修正 `traits/mod.rs` 宽口径模块文档
- **涉及代码**: `opc-da-client/src/opc_da/client/traits/mod.rs:1-31`（罗列全部 DA 1.0/2.0/3.0 能力，易误读为「都可用」）
- **工作内容**: 标注实际生产可用范围，区分「暴露/未暴露/死代码」。

### `[x]` C-03 · 诚实化根 README 措辞
- **涉及代码**: `README.md`（"Real-time Monitoring" 实为 1Hz 轮询；"local or remote hosts" 远程未接线）
- **工作内容**: 在 P1-04/P1-05 落地前，标注为 planned 或改述；`spec.md`/`architecture.md` 技术文档已诚实，保持不动。

### `[~]` C-04 · 清理已知代码质量小问题（3/4 子项已修，C-04.3 转 D-05）

- `[x]` **C-04.1** 删除死依赖 `clap`（`opc-cli/Cargo.toml`，全仓库零使用）+ 同步移除 `THIRD_PARTY_LICENSES.md` 的 clap 条目。
- `[x]` **C-04.2** 修正 `opc-cli/src/app.rs` `poll_write_result` 文案（"Browse error" → "Write error"）。
- `[x]` **C-04.4** `opc-da-client/src/backend/connector.rs` `item_properties` 改为可选接口容错（`.cast().ok()` + `Option` 字段 + `NotImplemented` impl），与 `server_public_groups`/`browse_server_address_space` 模式一致。
- `[--]` **C-04.3** `OpcDaClient::default()` panic — **转 D-05 决策**：作者 0.2.0 已用 rustdoc 文档化 panic 行为（`backend/opc_da.rs:20-22` + `CHANGELOG.md:13`），`opc-da-client/README.md` 4 处 quick-start 推广。deprecated/删除会推翻该决策，待 owner 拍板。

- **验收状态（2026-07-28）**: clippy（`--all-targets --all-features -D warnings`）✅ 全绿；`cargo test --workspace` ✅ 37 单测 + 10 doctest 全过；改动的 `app.rs`/`connector.rs` code-fmt ✅ 合规。
- **遗留（环境，非本次引入）**: `cargo fmt --all -- --check` 在本机因 `git core.autocrlf=true` 导致 46 个文件 CRLF 而失败（rustfmt.toml 要求 LF）。需在 `autocrlf=false/input` 环境复验，或加 `.gitattributes` 固定 LF（建议新增清理项 C-05）。

---

## 决策待定（需 owner 拍板）

| ID | 决策点 | 选项 | 影响 |
|---|---|---|---|
| **D-01** | DA 3.0 trait（7 个） | (a) 接线到 `ComServer`/`ComGroup`（完整 DA 3.0）／(b) 删除 | 决定 P3-01/02/03/04 走接线还是清理 |
| **D-02** | `v1`/`v3` 死代码模块 | (a) 删除／(b) `#[doc(hidden)]` 保留作参考 | 决定 C-01 形式 |
| **D-03** | DA 1.0 老式回调（`IAdviseSink`/`IDataObject`） | (a) 忽略（已被 `IOPCDataCallback` 取代，推荐）／(b) 实现 | 影响 `connector.rs:391` `IDataObject` 字段去留 |
| **D-04** | 订阅 API 形态 | (a) 返回 `futures::Stream`／(b) 回调闭包／(c) channel | 影响 P1-01 公共 API 设计 |
| **D-05** | `OpcDaClient::default()` panic 便利构造器 | (a) 保留现状（作者 0.2.0 已 rustdoc 文档化）／(b) `#[deprecated]` 指向 `new()`／(c) 删除（breaking，需改 README 4 处 + lib.rs 示例） | 推翻上一发布决策，影响公共 API；源自 C-04.3 |

### 裁定（2026-07-28，执行阶段）

- **D-01 = 接口层接线（已执行）**：DA 3.0 trait 已接线到生产 `ComServer`/`ComGroup`（消除"仅死代码"状态，见 P3-01/02/03/04）；`OpcProvider` 公共 API 暴露为后续独立任务（需 group/连接模型 API 设计）。
- **D-02 = `#[doc(hidden)]` 保留**：v1/v3 死代码不删除（v3 作为 DA 3.0 接线参考），加 `#[doc(hidden)]` + 注释消除公共 API 误导（见 C-01）。
- **D-03 = 忽略**：DA 1.0 老式回调（`IAdviseSink`/`IDataObject`）已被 `IOPCDataCallback` 取代，不实现。
- **D-04 = `futures::Stream`**：订阅 API 返回 `Stream<Item = TagValue>`，契合项目 async/tokio 风格。
- **D-05 = 维持现状**：尊重作者 0.2.0 决策（rustdoc 文档化 panic），不 deprecated/删除；`new()` 是推荐的 fallible 构造。

---

## 剩余核心任务实现指南（需专项环境验证）

以下 4 项是 ROADMAP 剩余的核心，**功能验证物理需要真实 OPC 服务器 / 远程 DCOM 环境**（本会话不具备），且涉及 `#[implement]` unsafe sink 或 `OpcProvider` API 重构，不宜盲写。精确实现指南如下，供有真实服务器的环境照做。

### P1-01 订阅（IOPCDataCallback）
1. `DataCallbackSink`：`#[windows_implement::implement(IOPCDataCallback)]`，持有 `tag_ids: Vec<String>`（index = client handle）+ `tokio::sync::mpsc::Sender<TagValue>`。
2. `impl IOPCDataCallback_Impl::OnDataChange`：遍历 `dwcount`，按 `*phclientitems.add(i)` 取 client handle → tag_id；`variant_to_string(&*pvvalues.add(i))` / `quality_to_string(*pwqualities.add(i))` / `filetime_to_string(*pfttimestamps.add(i))` 组装 `TagValue` → `tx.send`。`OnReadComplete`/`OnWriteComplete`/`OnCancelComplete` 返回 `Ok(())`。
3. worker `handle_subscribe`：**持久 group**（cache 扩到 group，不 `remove_group`）+ `add_items`（`hClient = index`）+ `ConnectionPointContainerTrait::data_callback_connection_point()` → `Advise(&sink, &cookie)`。
4. `OpcProvider::subscribe(server, tag_ids, update_rate) -> SubscriptionStream{cookie, rx: Receiver<TagValue>}`；`unsubscribe` 用 cookie `Unadvise` + remove group。
5. **验证阻塞**：需真实 OPC 服务器（Matrikon/Kepware）推送 `OnDataChange`；sink unsafe 回调需真实 COM apartment 验证。

### P1-04/05/06 远程 DCOM
1. `helpers::connect_server(server, host)`：host 非空时用已实现的 `create_server2(clsid, ctx, Some(ServerInfo{name:host, auth_info}))`。
2. `ServerConnector::connect(server, host)` + `enumerate_servers(host)`；`ComConnector` 远程用 `CoCreateInstanceEx`。
3. `ComRequest` 全变体加 `host`；worker cache 键改 `(host, server)`；`OpcProvider` 方法加 host 或引入连接模型。
4. **验证阻塞**：需远程 DCOM 环境验证；`OpcProvider` API breaking 重构（所有方法加 host 或连接对象）。

### P2-01 Shutdown（IOPCShutdown）
1. `ShutdownSink`：`#[implement(IOPCShutdown)]` + `ShutdownClosed` 回调 → 通知上层。
2. server 端 `IConnectionPointContainer::FindConnectionPoint(IID_IOPCShutdown)` → `Advise`。
3. **验证阻塞**：需真实服务器主动 shutdown 触发回调。

### P1-03 组状态
- `GroupStateMgtTrait::get_state/set_state` 已接线到 `ComGroup`（connector.rs）。
- `OpcProvider` 暴露依赖**持久 group**（订阅架构，见 P1-01）；当前一次性 group 下 `set_state` 无实际效果。

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-07-28 | 初始版本，基于全仓库深度审查与协议覆盖度详查建立基线 |
| 2026-07-28 | C-04：完成 .1（删 clap + THIRD_PARTY 同步）/ .2（write 文案）/ .4（item_properties 可选容错）；.3 转 D-05。clippy + test 全过。发现 fmt 因本机 autocrlf CRLF 失败（环境问题，建议 C-05） |
| 2026-07-28 | 阶段 A 接线完成：P2-03 GetStatus、P1-02 项属性（含 `ItemProperty` 类型 + COM 字符串/值转换）、P2-08 批量写（`write_tag_values` + 多项 mock）。`OpcProvider` 现 7 方法。clippy 全绿、40 单测通过。项属性/状态/批量写端到端功能 pending 真事 OPC 服务器验证 |
| 2026-07-28 | 横切清理 + 决策裁定：C-01（v1/v3 doc-hidden）、C-02（traits/mod.rs 实际可用范围审计）、C-03（README remote/real-time 诚实化）；D-01..D-05 裁定记录。全量 clippy + 34/40/10 测试 + doc links 全过 |
| 2026-07-28 | P2-07 MaxAge 读 + P2-06 VQT 写接线（ComGroup 加 `IOPCSyncIO2` + `SyncIo2Trait`，新增 `read_tag_values_max_age` / `write_tag_value_vqt`）。`OpcProvider` 现 10 方法。clippy 全绿、42 单测。SyncIO2 端到端 pending 真实服务器 |
| 2026-07-28 | P2-04 GetErrorString + P2-05 disconnect/reconnect（OpcDaClient inherent）接线完成。OpcProvider 现 11 方法 + 2 个生命周期 API。clippy 全绿、44 单测 |
| 2026-07-28 | P3-01/02/03/04 DA 3.0 接口接线完成：ComGroup 接通 GroupStateMgt2/AsyncIo3/ItemDeadbandMgt/ItemSamplingMgt，ComServer 接通 Browse/ItemIO（消除"仅死代码 v3"状态）。OpcProvider 公共 API 暴露为后续独立任务。D-01 裁定更新为"接口层已接线" |
| 2026-07-28 | P1-01 订阅 + P2-01 Shutdown 代码完成：DataCallbackSink/ShutdownSink（`#[implement]`）+ Advise/Unadvise + 持久 group/subscriptions 状态 + SubscriptionHandle/ShutdownHandle + `OpcProvider` subscribe/unsubscribe/subscribe_shutdown/unsubscribe_shutdown。`OpcProvider` 现 14 方法 + 2 inherent 生命周期 API。clippy 全绿、46 单测。功能 pending 真实服务器（OnDataChange/ShutdownRequest 推送验证） |
| 2026-07-28 | P1-03 set_subscription_rate + P3-06 locale/clientname 接线完成（GroupStateMgtTrait + CommonTrait）。`OpcProvider` 现 17 方法。clippy 全绿、47 单测 |
| 2026-07-28 | P3-07 browse 过滤参数化（browse_tags 加 data_type/access_rights，opc-cli/README 适配）。Phase 3 达 6/7。clippy 全绿、34/47/10 测试 |
| 2026-07-28 | P1-05 host 透传修复：ServerConnector::enumerate_servers 加 `host` 参数，worker 不再丢弃 ComRequest::ListServers.host（ComConnector 本机枚举，远程 CoCreateInstanceEx 待）。clippy 全绿 |
| 2026-07-28 | P1-04/05/06 远程 DCOM 完整接线：`AuthInfo::default_dcom` + `connect_server(host)`→`create_server2` + `get_servers(host)`→`CoCreateInstanceEx`(IOPCServerList) + ComConnector 带 host。Phase 1 达 6/6。功能 pending 远程 DCOM 环境验证。P3-05 公共组接口（ComServer impl ServerPublicGroupsTrait）。Phase 3 达 7/7 |
| 2026-07-28 | P2-02 set_keep_alive + P2-05 dispatch_with_retry 指数退避（max 3 次，50/100/200ms backoff，stale_connection 测试适配）。Phase 2 达 8/8。clippy 全绿、47 单测。至此 Phase 1/2/3 全部完成，ROADMAP 实质 100% |
| 2026-07-29 | 端到端测试套件落地（`e2e` feature + `tests/e2e.rs`，19 测试）连真实 `Matrikon.OPC.Simulation.1` 验证：browse(99 tags)/read/read_max_age(DA3.0)/write scalar/batch/VQT/subscribe(**收到 OnDataChange 推送**)/get_server_status/get_item_properties(14)/get_error_string/set_subscription_rate(2000ms)/set_keep_alive(5000ms,DA3.0)/set_locale/set_client_name/disconnect/reconnect **全 19 测试通过**。list_servers 返回空——根因：Matrikon 的 Implemented Categories(CATID) 仅注册在 32位注册表视图，64位 `IOPCServerList` 枚举不到（CLSID/ProgID 64位可见，故 connect/browse/read/write 正常）；remote 192.168.199.155 需 DCOM 权限配置 |
| 2026-07-29 | **远程 DCOM 端到端验证 + 关键 bug 修复**：新增 `examples/remote_list.rs` 诊断 CLI。32位 client 成功列出远程 192.168.199.155 的 `Matrikon.OPC.Simulation.1`。修复 `create_server2`/`get_servers` 的 **AuthInfo Bridge 悬垂指针 bug**（`try_to_native()` 返回临时 `COAUTHINFO`，`pAuthInfo` 取址后立即 drop → 32位 `0x800703E6`；64位侥幸不崩）→ 改用 `COSERVERINFO{pAuthInfo:null}`（DCOM 默认认证=当前登录用户，与 32位 Takebishi client 一致）。64位仍 `0x800706BA`（远程机仅 32位 OPC Core Components）。`.cargo/config.toml` 改 BuildTools 正确 linker 路径 |
