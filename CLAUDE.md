# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概览

`opc-cli` 是一个 Windows 平台的异步 TUI 客户端，用于浏览、读取、写入 OPC DA（Data Access）标签。基于 Rust 2024 edition（工具链 ≥ 1.93.1），核心依赖 Windows COM/DCOM，**只能在 Windows 上编译和运行**。

Cargo 工作区包含两个 crate：

- **`opc-cli`** — 交互式 TUI 二进制（`ratatui` + `crossterm` + `tokio`）。整个 crate `#![forbid(unsafe_code)]`。
- **`opc-da-client`** — OPC DA 通信库（`windows-rs`）。包含冻结的 COM 绑定与 vendor 合并代码，允许 `unsafe`。

## 常用命令

所有命令从**工作区根目录**运行。质量门用 `pwsh`（PowerShell 7），不是 `powershell`。

| 任务 | 命令 |
| :--- | :--- |
| 构建（debug） | `cargo build` 或 `make debug` |
| 运行 TUI | `cargo run --bin opc-cli` |
| 快速测试 | `cargo test --workspace` 或 `make test` |
| **完整质量门** | `pwsh -File scripts/verify.ps1` 或 `make verify` 或 `./verify.sh` |
| 格式检查 | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| 文档 | `cargo doc --no-deps --package opc-da-client` |
| 查看日志 | `make logs` 或 `pwsh -File scripts/check-logs.ps1` |

**跑单个测试**：`cargo test -p <crate> <test_name>`，例如 `cargo test -p opc-cli test_handle_key_event_press_release`。

**提交工作流**（先跑质量门再 commit+push）：`pwsh -File scripts/commit.ps1 -Message "<conventional commit>"`（等价 `make commit MSG="..."`）。

**打包发布**：现代 Win10+ 用 `make package`；遗留 Win7/Server 2008 R2 用 `make package-win7`。**不要绕过这些脚本手搓打包**——Win7 流程包含静态 CRT 链接、polyfill DLL 编译和 PE 导入表二进制 patch。

### verify.ps1 质量门做了什么

按顺序串行执行，**任一步非零退出即中止**：`cargo fmt --check` → `cargo clippy --workspace --all-targets --all-features -D warnings` → `cargo test --doc --workspace` → `cargo test --workspace` → 逐个独立编译 `compat/*/` polyfill crate。最后一步是必须的，因为 `compat/` 被排除在工作区之外（见下文）。

## 架构（必读）

完整设计见 `opc-da-client/architecture.md`（技术真相源）和 `opc-da-client/spec.md`（行为契约真相源）。注意：根 `README.md` 里的 `./architecture.md` 链接是错的，文件实际在 `opc-da-client/` 下。

### 1. 稳定 API + 可替换后端

`OpcProvider`（`opc-da-client/src/provider.rs`）是面向消费者的 async trait：`list_servers` / `browse_tags` / `read_tag_values` / `write_tag_value`。具体实现 `OpcDaClient` 在 `backend/opc_da.rs`，通过泛型 `ServerConnector`（`backend/connector.rs`）与真实 COM 解耦——这让单元测试能在任意 OS 上用 in-process mock 跑，无需真实 OPC 服务器。

- `opc-da-backend`（默认 feature）启用真实 COM 后端。
- `test-support` feature 通过 `mockall::automock` 生成 `MockOpcProvider`，供 `opc-cli` 的 UI/状态测试使用。

### 2. COM 线程模型（不要破坏线程亲和性）

OPC DA 的 COM 指针有严格的线程亲和性。`ComWorker`（`com_worker.rs`）的解决方案：

- `ComWorker::start()` 起一个专用 `std::thread`，在该线程内 `CoInitializeEx(MTA)` 并**独占所有 COM 指针**。
- async trait 方法把请求封装成 `ComRequest`，通过 tokio `mpsc` 发给 worker，结果经 `oneshot` 回传。
- worker 维护按 ProgID 索引的连接池（`HashMap<String, C::Server>`），`dispatch_with_retry` 在检测到 RPC 连接错误（如 `RPC_S_SERVER_UNAVAILABLE`）时驱逐失效代理、重连并重试。

**含义**：永远不要从 async 上下文直接碰 COM 指针——所有 COM 调用必须经 worker channel。新操作类型要加到 `ComRequest` 枚举 + worker `match` 分发。

### 3. TUI 事件循环与状态机

`opc-cli/src/main.rs` 的 `run_app` 是单线程 tick 循环：每轮 `poll_*_result()`（非阻塞 `oneshot::Receiver::try_recv`）→ 渲染 → 100ms 内取一个按键。`app.rs` 的 `App` 是状态机，`CurrentScreen` 枚举（`Home`/`Loading`/`ServerList`/`TagList`/`TagValues`/`WriteInput`/`Exiting`）驱动渲染和按键路由。每个长操作（list/browse/read/write/auto-refresh）模式一致：`start_*` 设状态为 `Loading` 并 `tokio::spawn` 一个带 `tokio::time::timeout` 的任务，`poll_*` 在后续 tick 收割结果。

### 4. Browse 策略

`browse_tags` 处理 flat 与 hierarchical 命名空间：先 `query_organization()` 判型；hierarchical 时优先尝试 `OPC_FLAT` 快速路径（一次性返回全部叶子），失败/空则递归回退（`browse_recursive`：先 branches 后 leaves，DOWN 后**总是** UP 回退以防位置腐蚀）。`tags_sink`（`Arc<Mutex<Vec<String>>>`）允许超时时收割部分结果；`max_tags`（默认 10000）和 `MAX_DEPTH`（50）做硬上限。

## 关键约定（违反会被质量门拦下）

- **禁止 panic**：生产代码中**不得**使用 `unwrap()` / `expect()` / 直接 panic。所有可失败函数返回 `OpcResult<T>`。
- **错误传播**：用 `.context()` / `.with_context()` 包裹；在传播前用 `map_err(|e| { tracing::error!(...); e })` 先记日志，以保留原始 HRESULT。
- **HRESULT 友好提示**：用 `friendly_com_hint()` / `format_hresult()`（`helpers.rs`）把晦涩的 COM/DCOM HRESULT 翻成可读串。
- **Clippy 极严**：工作区 `clippy::all = deny`、`pedantic`/`cargo`/`nursery = warn`、`undocumented_unsafe_blocks = deny`。部分项已 allow（见根 `Cargo.toml`）。提交前本地跑一遍 `make verify`。
- **unsafe 规则随 crate 而异**：`opc-cli` 是 `#![forbid(unsafe_code)]`；`opc-da-client` 允许 unsafe（COM 必需），但每个 unsafe 块**必须有 `// SAFETY:` 注释**解释不变量。
- **rustfmt**：`max_width=100`、4 空格、Unix 换行、忽略 `vendor/`（见 `rustfmt.toml`）。
- **文档**：公开项用 `///`，可失败函数必须有 `# Errors` 段。`bindings/`（`#[allow(warnings)]`）是冻结的 winmd bindgen 产物——**不要手改**。
- **日志**：`tracing`，输出到 `logs/opc-cli.log`（每日滚动）。级别约定见 `architecture.md` §6。可用 `RUST_LOG` 环境变量调过滤级别。

## 已知坑

- **`.cargo/config.toml` 配置 MSVC linker 路径**（已从作者机器特定的 portable-msvc 改为本机 BuildTools 正确路径，含 `x86_64` 与 `i686` 两个 target）。换机器构建若失败，先改这里指向你的 `link.exe`。
- **OPC 位宽（32/64）与 `IOPCServerList` 枚举**：OPC DA 服务器（Matrikon/Kepware）常只把 Implemented Categories(CATID) 注册在 **32位注册表视图**。64位 `IOPCServerList::EnumClassesOfCategories` 枚举 32位 CATID → 返回空（`list_servers` 空）。但 CLSID/ProgID 通常 32+64 位都有，所以 `connect`/`browse`/`read`/`write`（直连路径）正常。远程机的 OPC Core Components 也可能只装 32位。本机 e2e（64位）与远程 e2e（64/32位）均验证：list/browse/read/write 跨位宽工作；`list_servers` 按 CATID 枚举受位宽影响。
- **`AuthInfo`/`ServerInfo` Bridge 悬垂指针（已修）**：`ServerInfoBridge/AuthInfoBridge::try_to_native()` 曾返回临时 `COAUTHINFO`，`COSERVERINFO.pAuthInfo` 取其地址后临时立即 drop → 悬垂（64位侥幸不崩，32位 `0x800703E6` 内存访问无效）。远程分支（`create_server2`/`get_servers`）已改用 `COSERVERINFO{ pAuthInfo: null }`（DCOM 默认认证 = 当前登录用户，与 32位 Takebishi client 一致）。**如需自定义 DCOM 凭据**（特定用户/模拟级），必须正确管理 `COAUTHINFO`/`COAUTHIDENTITY` 的内存生命周期——不能再让 `try_to_native()` 返回临时再取址。
- **远程订阅（reverse-DCOM callback）**：`subscribe` 的 `IOPCDataCallback` sink 需服务器**反向回调**客户端。本机订阅工作（e2e 验证收到 OnDataChange）；远程订阅的 `Advise` 需客户端机器**允许入站 DCOM** + sink 可编组，否则报 `0x800706BA`（RPC unavailable）。这是 DCOM callback 配置，非库 bug。
- **`compat/` 被排除在工作区外**（根 `Cargo.toml` 的 `exclude`）。它们是 Win7/Server 2008 R2 遗留发布用的 polyfill crate（`#![no_std]`），改了之后 `cargo test --workspace` 不会编译它们——`verify.ps1` 单独逐个构建来兜底。
- **OPC-BUG-001 已修**（`StringIterator` 的 `E_POINTER` 洪流，已在 `next()` 中清缓存修复）；caller 端的 `is_known_iterator_bug()` workaround 已移除，不要再加回来。
- **DCOM 过滤是有意省略的**：`Client` 不过滤 `CATID_OPCDAServer10/20` 以免漏掉注册表元数据不全的服务器，多余的非 OPC GUID 在 `guid_to_progid` 转换阶段过滤。这不是 bug。
