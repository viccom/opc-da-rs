# OPC DA Client & Server（Rust）

> **语言**：[English](README.md) | 简体中文

面向 Windows 的现代 Rust 工作区——异步**客户端**（TUI + 桌面 GUI）用于浏览 / 读取 / 写入 / 订阅标签，外加原生 **OPC DA 服务器**库与对标 Matrikon 的**仿真服务器**，便于客户端测试与协议网关开发（Modbus / S7 / UA 桥接）。

## 🧩 组件

本仓库是一个 Cargo 工作区，包含六个 crate：

| Crate | 侧 | 说明 |
| :--- | :--- | :--- |
| **[`opc-cli`](./opc-cli/)** | 客户端 | 交互式 TUI 应用（`ratatui` + `crossterm`）。 |
| **[`opc-da-desktop`](./opc-da-desktop/)** | 客户端 | Tauri 2 桌面 GUI（React + TypeScript），用于浏览 / 订阅 / 写入。（[README](./opc-da-desktop/README.md)） |
| **[`opc-da-client`](./opc-da-client/)** | 客户端 | 异步 OPC DA 库——`OpcProvider` trait + 原生 `windows-rs` COM 后端，两个客户端共用。（[README](./opc-da-client/README.zh-CN.md)） |
| **[`opc-da-server`](./opc-da-server/)** | 服务器 | OPC DA Server COM 库——`IOPCServer` / `Group` / `ItemMgt` / `SyncIO` / `AsyncIO2` / `Browse` / `ItemProperties` + 全局推送调度器 + GIT 跨 apartment 回调。 |
| **[`opc-da-server-sim`](./opc-da-server-sim/)** | 服务器 | 仿真服务器（对标 `Matrikon.OPC.Simulation`），x64 + x86。（[README](./opc-da-server-sim/README.md)） |
| **[`opc-da-client-test`](./opc-da-client-test/)** | 测试 | 端到端测试程序，驱动 `opc-da-client` 对接 `opc-da-server` / `opc-da-server-sim`。 |

延伸阅读：高层导航图 **[ARCHITECTURE_DIAGRAM.md](./ARCHITECTURE_DIAGRAM.md)**、客户端库深设计 **[opc-da-client/architecture.md](./opc-da-client/architecture.md)**、仿真服务器指南 **[opc-da-server-sim/README.md](./opc-da-server-sim/README.md)**、同步/异步/订阅与 DCOM 详解 **[DCOM_GUIDE.md](./DCOM_GUIDE.md)**。

## 🏗️ 架构

本项目以 Cargo 工作区组织，横跨 OPC DA 的客户端与服务器两侧：

- **客户端**（`opc-cli`、`opc-da-desktop`）基于 **`opc-da-client`** 库——原生 Windows COM 库（`windows-rs`），通过 async trait（`OpcProvider`）抽象 OPC DA 通信，泛型于 `ServerConnector` 便于 mock。
- **服务器**（`opc-da-server-sim`）基于 **`opc-da-server`** 库——原生 OPC DA Server COM 实现（`IOPCServer` / `Group` / `SyncIO` / `AsyncIO2` / `Browse`…），含全局推送调度器与 Global Interface Table（GIT）跨 apartment 回调，让 STA 客户端（PsOPCClient、Prosys）收 `OnDataChange` 不报 `RPC_E_WRONG_THREAD`。

完整客户端设计见 **[opc-da-client/architecture.md](./opc-da-client/architecture.md)**，服务器侧接线见 **[opc-da-server-sim/README.md](./opc-da-server-sim/README.md)**。

## ✨ 功能特性

- **服务器发现**：枚举本机或远程主机（DCOM）上的 OPC DA 服务器，可选附带 CLSID + 厂商描述（`list_servers_with_details`）。
- **显式 DCOM 凭据**：用独立的 user/password/domain 认证远程服务器——适用于跨域或服务账户访问。
- **分层浏览**：递归探查复杂服务器命名空间，支持超时收割部分结果、数据类型/访问权限过滤，以及惰性单层浏览（`browse_children`）。
- **实时订阅**：经 `IOPCDataCallback` 的事件驱动数据变更通知（真正的推送，非轮询），并在回调静默失效时自愈。TUI 另支持 1 秒自动刷新轮询作为兜底。
- **融合读取器**：推送优先 / 轮询兜底的流（`FusionReader`），使被拦的反向 DCOM 回调绝不会让消费方拿到过期数据——桌面端使用。
- **标签读写**：同步读、MaxAge 读（`IOPCSyncIO2`）、VQT 写（值 + 质量 + 时间戳）、批量操作。
- **诊断**：服务器状态（`IOPCServer::GetStatus`）、项属性（EU/数据类型/访问权限）、本地化错误串、服务器关停通知。
- **连接韧性**：连接池 + 失效代理驱逐 + 指数退避重连 + 显式 disconnect/reconnect API。
- **搜索与过滤**：子串搜索，`Tab`/`Shift+Tab` 在匹配项间循环。
- **友好的错误提示**：把晦涩的 Windows COM/DCOM HRESULT 翻成可读说明。
- **COM 管理透明**：COM 初始化与 apartment 线程亲和性由专用后台 worker 线程自动处理。
- **可 mock 的后端**：无需真实 OPC 服务器即可在任何 OS 上对 TUI 做单元测试。

## 🚀 快速开始

### 前置条件

- **Windows 操作系统**：本应用使用 Windows COM/DCOM。
- **OPC Core Components**：须安装以解析 OPC ProgID。
- **Rust 1.93+**：Edition 2024。

### 构建与运行

```powershell
# 运行 TUI
cargo run --bin opc-cli

# 运行完整质量门（格式 → lint → 测试）
pwsh -File scripts/verify.ps1
```

## 🖥️ OPC DA Server（`opc-da-server-sim`）

`opc-da-server-sim` 是独立的 OPC DA 仿真服务器，镜像 `Matrikon.OPC.Simulation` 标签集——无需真实 PLC 即可开发/测试客户端，或作为构建自定义协议网关服务器（Modbus / S7 / UA 桥接）的模板。它是 `opc-da-server` 库的薄包装，完整示范「接入自定义 `DataSource` → 注册 → 运行」流程。

**注册与运行**（管理员提示符）：

```powershell
# 64 位构建——面向 64 位 OPC 客户端
target\release\opc-da-server-sim.exe /RegServer

# 32 位构建——32 位桌面客户端（PsOPCClient、Prosys OPC Client）必需
cargo build --release -p opc-da-server-sim --target i686-pc-windows-msvc
target\i686-pc-windows-msvc\release\opc-da-server-sim.exe /RegServer
```

随后用任意标准 OPC DA 客户端连接 ProgID **`opc-da-rs.Sim.1`**（hierarchical 命名空间，如 `Random.Int4.0`、`BucketBrigade.Int4.0`、`_System.Time`）。通过 `opc-da-server-sim.ini` 配置规模（`count = N`，上限 100 000 → 约 80 万 tag）。

> ⚠ **位宽**：32 位客户端连 64 位服务器，若 `System32\OPCProxy.dll`（64 位）缺失，跨位宽 marshaling 可能崩溃——请让服务器位宽匹配客户端，或安装 64 位 OPC Core Components。详见 [opc-da-server-sim/README.md](./opc-da-server-sim/README.md)。

CI 构建**两种**位宽，在每个 `v*` tag 的 GitHub Release 上发布 `opc-da-server-sim-x64.exe` + `opc-da-server-sim-x86.exe`（+ `opc-da-server-x64.exe`）。

## ⌨️ 按键操作

| 按键 | 动作 | 屏幕 |
| :--- | :--- | :--- |
| `Enter` | 前进 / 确认输入 | 全部 |
| `Esc` | 后退 | 全部 |
| `Space` | 切换标签选中 | 标签列表 |
| `s` | 进入搜索/过滤模式 | 标签列表 |
| `Tab` / `Shift+Tab` | 在搜索匹配间循环 | 标签列表（搜索） |
| `w` | 进入所选标签的写入模式 | 标签值 |
| `↑` / `↓` | 列表导航 | 所有列表 |
| `PgUp` / `PgDn` | 翻页（每页 20 项） | 所有列表 |
| `q` / `Q` | 退出应用 | 主页 |

## 📦 打包与部署

本仓库支持两种发布打包模式：

### 1. 现代发布（Windows 10+ / Server 2016+）

```powershell
make package
# 或
pwsh -File scripts/package.ps1 package
```
产物：`dist/opc-cli-x64/` 与 `dist/opc-cli-x64.zip`

### 2. 遗留发布（Windows 7 SP1 / Server 2008 R2 SP1）

面向运行 Windows 7 / Server 2008 R2（NT 6.1）的离线、气隔工业环境部署：

```powershell
make package-win7
# 或
pwsh -File scripts/package.ps1 package-win7
```
产物：`dist/opc-cli-win7-x64/` 与 `dist/opc-cli-win7-x64.zip`

**遗留包内容：**
- `opc-cli.exe`：经 PE patch、静态链接 CRT（`+crt-static`）的可执行文件。把缺失的 `GetSystemTimePreciseAsFileTime` 导入替换为原生 `GetSystemTimeAsFileTime`。
- `api-ms-win-core-synch-l1-2-0.dll`：`WaitOnAddress` 与 `Sleep` 重导出的 `#![no_std]` polyfill。
- `api-ms-win-core-winrt-error-l1-1-0.dll`：WinRT 错误 API 的 `#![no_std]` 空实现桩。
- `bcryptprimitives.dll`：将 `ProcessPrng` 路由到 `RtlGenRandom`（`advapi32.dll`）的 `#![no_std]` polyfill。
- `redist/`：附带的 OPC Core Components 可再分发 MSI（若放置于 `vendor/redist/`）。

把解压后的 `dist/opc-cli-win7-x64/` 文件夹拷到 U 盘，即可在目标机器上运行，无需安装 Visual C++ 可再分发组件或 Windows 更新。

## 🙏 致谢

- [**rust_opc**](https://github.com/Ronbb/rust_opc)（Wang Ruobiao）——原始 OPC DA Rust 绑定与 COM 接口生成流水线。
- [**OPC Foundation**](https://opcfoundation.org/)——OPC Data Access 规范与 IDL 接口定义。
- [**windows-rs**](https://github.com/microsoft/windows-rs)（Microsoft）——面向 Rust 的 Windows API 绑定。
- [**ratatui**](https://github.com/ratatui/ratatui)——终端用户界面框架。

## 📄 许可

本项目基于 MIT 许可授权——详见 [LICENSE](LICENSE)。
