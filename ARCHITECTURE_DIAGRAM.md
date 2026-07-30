# Architecture Diagram

> **TL;DR**: 一张图 + 一棵树 + 一条流。说明 crate 分层、模块归属、请求路径。
>
> 技术真相源在 [`opc-da-client/architecture.md`](opc-da-client/architecture.md)；
> 行为契约真相源在 [`opc-da-client/spec.md`](opc-da-client/spec.md)。
> 本文件只做**导航**，不重复细节。

---

## 1. Workspace 总览

```
                        ┌────────────────────────────────────────────┐
                        │             Cargo Workspace (root)          │
                        │   Cargo.toml (exclude = compat, target, …)  │
                        └────────────────────┬───────────────────────┘
                                             │
              ┌──────────────────────────────┴───────────────────────────────┐
              │                                                              │
              ▼                                                              ▼
   ┌────────────────────────┐                                 ┌──────────────────────────────┐
   │      opc-cli           │                                 │       opc-da-client         │
   │  交互式 TUI 二进制     │                                 │   OPC DA 通信库             │
   │  ratatui + crossterm   │                                 │   windows-rs / COM / DCOM   │
   │  #![forbid(unsafe)]    │                                 │   允许 unsafe（每块带注释） │
   │                        │                                 │                              │
   │  src/                  │                                 │  3 个 feature:               │
   │   main.rs   ─ 事件循环 │                                 │   • opc-da-backend  (默认)   │
   │   app.rs    ─ 状态机   │                                 │   • test-support   (mock)    │
   │   ui.rs     ─ 渲染     │                                 │   • e2e            (真机)   │
   └─────────────┬──────────┘                                 └──────────────┬───────────────┘
                 │                                                           │
                 │  消费                                                     │  暴露
                 ▼                                                           ▼
              OpcProvider (trait, 18 方法)  ◀────────────────────────────  OpcDaClient<C: ServerConnector>
                                                                                  │
                                                                                  ▼
                                                                            ComConnector (持有 host)
                                                                                  │
                                                                                  ▼
                                                                            ComWorker (专用 COM 线程)
                                                                                  │
                                                                                  ▼
                                                                            ComServer / ComGroup
                                                                                  │
                                                                                  ▼
                                                                            bindings/  (winmd 冻结)
```

**两条关键约束**：

- `opc-cli` **永远不**直接碰 COM 指针——所有 OPC 调用经 `OpcProvider`。
- `OpcProvider` 是稳定 API；`OpcDaClient` 是可替换实现。未来增加 OPC UA / 其它后端不动 trait。

---

## 2. 目录树（不含 `target/` / `.git/`）

```
opc-cli/                                      # 工作区根
├── .cargo/config.toml                        # MSVC linker 路径（x64 + i686）
├── CLAUDE.md                                 # 项目 AI 指令
├── ROADMAP.md / README.md                    # 任务方向 + 项目门面
├── ARCHITECTURE_DIAGRAM.md                   # 本文件
├── Cargo.toml / Cargo.lock                   # 工作区清单（exclude = ["compat", …]）
├── Makefile                                  # verify / test / build / package / commit
├── verify.sh                                 # bash 入口 → pwsh scripts/verify.ps1
├── rustfmt.toml                              # max_width=100, 4 空格, Unix LF
├── THIRD_PARTY_LICENSES.md
├── LICENSE
│
├── scripts/                                  # 质量门与发布脚本（pwsh）
│   ├── verify.ps1                            # fmt → clippy → doctest → test → compat 构建
│   ├── commit.ps1                            # verify 后 commit + push
│   ├── check-logs.ps1
│   ├── package.ps1 / package-win7.ps1        # 现代 Win10+ 与遗留 Win7/2008 R2 打包
│   └── Merge-ToMain.ps1
│
├── logs/                                     # 运行日志（按日滚动 opc-cli.log.YYYY-MM-DD）
├── vendor/redist/README.md                   # 三方可再分发说明（不要 commit 实体 .dll）
│
├── opc-cli/                                  # 二进制 crate
│   ├── Cargo.toml
│   ├── logfile_format.md
│   └── src/
│       ├── main.rs                           # run_app tick 循环
│       ├── app.rs                            # App + CurrentScreen 状态机
│       └── ui.rs                             # ratatui 渲染
│
└── opc-da-client/                            # 库 crate（version = 0.3.0）
    ├── Cargo.toml                            # features: opc-da-backend / test-support / e2e
    ├── README.md                             # crates.io readme
    ├── CHANGELOG.md                          # 0.1.0 → 0.3.0（Keep-a-Changelog）
    ├── architecture.md                       # 技术真相源
    ├── spec.md                               # 行为契约真相源
    ├── .winmd/                               # OPCDA.winmd + OPCCOMN.winmd（输入给 bindgen）
    ├── rewrite.py                            # bindgen 辅助脚本（不再主动调用）
    ├── examples/
    │   └── remote_list.rs                    # 远程 DCOM 枚举的诊断 CLI
    ├── tests/
    │   └── e2e.rs                            # 19 本地 + 4 远程 e2e（feature = e2e）
    │
    └── src/
        ├── lib.rs                            # 公开导出面（prelude + re-exports）
        ├── provider.rs                       # async trait OpcProvider（18 方法契约）
        ├── subscription.rs                   # SubscriptionHandle + rx 异步流
        │
        ├── com_guard.rs                      # RAII：CoInitializeEx(MTA) 生命周期
        ├── com_worker.rs                     # 专用 COM 线程 + mpsc/oneshot 请求分发 + 池化重连
        ├── helpers.rs                        # friendly_com_hint() / format_hresult()
        │
        ├── backend/                          # 适配层（trait 注入以解耦真实 COM）
        │   ├── mod.rs
        │   ├── opc_da.rs                     # OpcDaClient<C: ServerConnector>
        │   └── connector.rs                  # ServerConnector trait + ComConnector 实现
        │
        ├── bindings/                         # ⚠ 冻结的 winmd bindgen 产物（#[allow(warnings)]）
        │   ├── mod.rs
        │   ├── comn/{mod.rs, bindings.rs}    # IOPCCommon 等 OPC Common 接口
        │   └── da/{mod.rs, bindings.rs}      # IOPCServer 等所有 OPC DA 接口
        │
        └── opc_da/                           # COM 类型化封装
            ├── mod.rs                        # 模块入口 + 重导出
            ├── com_utils.rs                  # VariantClear / RemotePointer / SafeArray
            │                                 #   含 clear_variant_array / clear_item_states
            ├── errors.rs                     # OpcError + From<HRESULT>
            ├── typedefs.rs                   # tag 结构体浅包装
            │
            └── client/
                ├── mod.rs                    # ComServer / ComGroup 聚合
                ├── iterator.rs               # StringIterator（OPC-BUG-001 已修）
                ├── v1/mod.rs                 # DA 1.0 入口
                ├── v2/mod.rs                 # DA 2.0 增量
                ├── v3/mod.rs                 # DA 3.0 增量
                │
                └── traits/                   # 每个 COM 接口一个文件
                    ├── mod.rs
                    ├── server.rs                  # IOPCServer
                    ├── server_public_groups.rs    # IOPCServerPublicGroups  (DA 3.0)
                    ├── common.rs                  # IOPCCommon
                    ├── browse_server_address_space.rs  # IOPCBrowseServerAddressSpace
                    ├── browse.rs                  # IOPCBrowse                  (DA 3.0)
                    ├── item_mgt.rs                # IOPCItemMgt
                    ├── group_state_mgt.rs         # IOPCGroupStateMgt
                    ├── group_state_mgt2.rs        # IOPCGroupStateMgt2          (DA 3.0)
                    ├── public_group_state_mgt.rs  # IOPCPublicGroupStateMgt
                    ├── sync_io.rs                 # IOPCSyncIO
                    ├── sync_io2.rs                # IOPCSyncIO2  (ReadMaxAge / WriteVQT, DA 3.0)
                    ├── async_io.rs                # IOPCAsyncIO
                    ├── async_io2.rs               # IOPCAsyncIO2
                    ├── async_io3.rs               # IOPCAsyncIO3                (DA 3.0)
                    ├── connection_point_container.rs  # IConnectionPointContainer（订阅入口）
                    ├── data_object.rs             # IDataObject
                    ├── item_properties.rs         # IOPCItemProperties
                    ├── item_io.rs                 # IOPCItemIO                  (DA 3.0)
                    ├── item_deadband_mgt.rs       # IOPCItemDeadbandMgt        (DA 3.0)
                    └── item_sampling_mgt.rs       # IOPCItemSamplingMgt        (DA 3.0)
```

> ⚠ `bindings/` 是 `#[allow(warnings)]` 冻结产物——**不要手改**；改 OPC 类型层请改 `opc_da/` 之上的封装。

---

## 3. 请求路径（一次 `read_tag_values` 调用到底层 COM）

```
opc-cli (Tokio task)
   │
   │  app.start_read(...)        spawn 一个 tokio::time::timeout 包裹的任务
   ▼
OpcProvider::read_tag_values     ←── async trait, 18 方法契约的入口
   │
   ▼
OpcDaClient<C>::read_tag_values  ←── backend/opc_da.rs
   │  1. 选/建连接（dispatch_with_retry，按 ProgID 池化）
   │  2. 把请求包成 ComRequest::Read
   │  3. tokio::mpsc::Sender.send(req)
   ▼
ComWorker (专用 std::thread)
   │  CoInitializeEx(MTA)  ←─ 只在这里初始化 COM
   │  match ComRequest → 调用对应 ComServer/ComGroup 方法
   │  读取 IOPCSyncIO::Read
   │  ↓
   │  关键修复 (v0.3.0 P0)：
   │  拿到 RemoteArray<tagOPCITEMSTATE> 后
   │  clear_item_states(&mut arr)        ←── VariantClear 每个 vDataValue
   │  然后再交给 RemotePointer::drop     ←── CoTaskMemFree 数组缓冲
   │  结果打包进 oneshot::Sender
   ▼
回到 tokio 任务
   oneshot::Receiver.await → TagValue 列表 → 投递给 app
```

**为什么这层重要**：`RemotePointer::drop` 只 `CoTaskMemFree` 数组缓冲，**不**做 `VariantClear`——
若不显式遍历清理，嵌入的 BSTR/SafeArray 会在每次读/写时泄漏。`com_utils.rs::clear_variant_array` / `clear_item_states` 是这条路径唯一的兜底点。

---

## 4. 关键不变量（违反 = 设计破坏）

| 不变量 | 守护位置 |
| --- | --- |
| COM 指针只在专用线程接触 | `com_worker.rs`（唯一 `CoInitializeEx`） |
| `OpcProvider` 是稳定 API（不动 trait） | `provider.rs`（扩展走新方法，不改旧签名） |
| 生产代码不 panic | workspace `clippy::all = deny` + `unwrap_used = deny`（见根 `Cargo.toml`） |
| 每个 `unsafe` 块必须有 `// SAFETY:` | `clippy::undocumented_unsafe_blocks = deny` |
| `bindings/` 不可手改 | `#[allow(warnings)]` + 项目约定 |
| 跨平台测试可跑 | `ServerConnector` trait 让 `MockServerConnector` 注入；CI 无需真 OPC |
| `compat/` 不进工作区 | 根 `Cargo.toml` 的 `exclude`；`verify.ps1` 单独兜底构建 |

---

## 5. 维护备忘

- 新增 OPC 操作 → 在 `OpcProvider` 加方法 + 在 `ComRequest` 加枚举分支 + 在 worker `match` 加处理 + 在 `OpcDaClient` 加分发。
- 新增 COM 接口 → 在 `bindings/` 生成 → 在 `opc_da/client/traits/` 加 trait 文件 → 在 `v1/v2/v3/mod.rs` 选择挂载版本。
- 远程 DCOM 改动 → 注意 `COSERVERINFO.pAuthInfo = null`（v0.3.0 P0 修复）；自定义凭据需要自己管理 `COAUTHINFO`/`COAUTHIDENTITY` 生命周期。
- 测试金字塔：unit（mockall）+ workspace `cargo test` + 真机 `cargo test --features e2e`（仅 Windows + 真实 OPC 服务器）。