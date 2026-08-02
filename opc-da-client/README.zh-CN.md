# opc-da-client

[![Crates.io](https://img.shields.io/crates/v/opc-da-client.svg)](https://crates.io/crates/opc-da-client)
[![Docs.rs](https://docs.rs/opc-da-client/badge.svg)](https://docs.rs/opc-da-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **语言**：[English](README.md) | 简体中文
>
> **镜像**：亦同步于 [Gitea](https://git.metme.top/viccom/opc-da-lib)，主开发在 [GitHub](https://github.com/viccom/opc-da-rs)。

面向 Rust 的后端无关 OPC DA 客户端库——异步、基于 trait、COM 管理对调用方透明。

## 功能特性

- **异步 API**：基于 `tokio` 与 `async-trait`，为现代异步 Rust 而生。
- **基于 trait 的抽象**：`OpcProvider` trait 便于 mock 与替换后端。
- **COM 管理透明**：在后台自动完成 COM 初始化（`CoInitializeEx`）与线程亲和性（apartment）管理。
- **OPC DA 全覆盖**：浏览、读、写、订阅（`IOPCDataCallback`）、关停通知（`IOPCShutdown`）、服务器状态、项属性、MaxAge 读取、VQT 写入、批量操作，以及 DA 3.0 接口。
- **远程 DCOM**：经 DCOM（`CoCreateInstanceEx`）连接远程主机上的 OPC DA 服务器。
- **显式 DCOM 凭据**：用独立的 user/password/domain 认证远程服务器（`AuthCredentials` + `OpcDaClient::with_credentials`）——适用于跨域或服务账户访问当前登录用户够不到的场景。`user` 留空则退化为当前登录用户（DCOM 默认认证）。
- **融合读取器（Fusion Reader）**：`FusionReader` 优先走真正的推送订阅，当反向 DCOM 回调打不通（入站 DCOM 被拦、防火墙严格）时自动回退到同步轮询——一条流，绝不静默断流。
- **服务器富信息与状态**：`list_servers_with_details` 用 CLSID + 厂商描述补充服务器列表（`IOPCServerList::GetClassDetails`）；`get_server_status` 暴露运行时状态、版本、厂商、组数与时间戳（`IOPCServer::GetStatus`）。
- **连接韧性**：连接池 + 失效代理驱逐 + 指数退避重连 + 显式 disconnect/reconnect API。
- **原生 Windows COM/DCOM**：经 `windows-rs` 的原生 OPC DA 后端——无需任何外部 OPC crate。
- **健壮的错误处理**：以 `thiserror` 定义领域类型 `OpcError`，用 `friendly_com_hint()` 把 HRESULT 翻成可读提示。
- **测试友好**：`test-support` feature 经 `mockall` 提供 `MockOpcProvider`；`e2e` feature 提供针对真实 Matricon/Kepware 服务器的端到端测试集。

## 安装

在你的 `Cargo.toml` 中添加：

```toml
[dependencies]
opc-da-client = "0.3"
```

> **注——尚未发布的功能。** DCOM 凭据、`FusionReader`、`list_servers_with_details`
> 位于 `main` 分支，**尚未发布到 crates.io**（最新已发布版本为 `0.3.1`）。如需立即使用，
> 请依赖 Git 版本：
>
> ```toml
> opc-da-client = { git = "https://github.com/viccom/opc-da-rs" }
> ```

## 前置条件

- **操作系统**：Windows（COM/DCOM 是 Windows 专有技术）。
- **OPC DA Core Components**：须在本机安装并注册 OPC DA 核心组件。
- **DCOM 配置**：连接远程服务器时须配置相应的 DCOM 权限。同步/异步/订阅三种机制的通讯方向与认证差异、本机 vs 远程的配置与排障，见工作区根目录的 [`DCOM_GUIDE.md`](../DCOM_GUIDE.md)。

## 用法示例

### 连接并枚举服务器

枚举本机或远程主机上可用的 OPC DA 服务器。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();

    let servers = client.list_servers("localhost").await?;
    println!("可用服务器：");
    for server in servers {
        println!("  - {}", server);
    }
    Ok(())
}
```

### 枚举服务器（含富信息）

`list_servers_with_details` 经 `IOPCServerList::GetClassDetails` 为每个条目补充 CLSID 与厂商用户类型描述。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();

    for s in client.list_servers_with_details("localhost").await? {
        println!(
            "{}  [{}]  {}",
            s.prog_id,
            s.clsid,
            s.user_type.unwrap_or_default(),
        );
    }
    Ok(())
}
```

### 读取标签

连接到指定服务器，读取一组标签的当前值。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server_progid = "Matrikon.OPC.Simulation.1";
    let tags = vec![
        "Random.Int4".to_string(),
        "Random.Real8".to_string(),
    ];

    let values = client.read_tag_values(server_progid, tags).await?;

    for v in values {
        println!("Tag: {}, Value: {}, Quality: {}, Time: {}",
            v.tag_id, v.value, v.quality, v.timestamp);
    }
    Ok(())
}
```

### 写入值

向单个 OPC 标签写入一个类型化值。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider, OpcValue};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server = "Matrikon.OPC.Simulation.1";

    let result = client
        .write_tag_value(server, "Bucket Brigade.Int4", OpcValue::Int(42))
        .await?;

    if result.success {
        println!("✓ 写入成功");
    } else {
        println!("✗ 写入失败：{}", result.error.as_deref().unwrap_or("未知错误"));
    }
    Ok(())
}
```

### 浏览地址空间

递归发现 OPC 服务器上的可用标签。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};
use std::sync::{Arc, Mutex, atomic::AtomicUsize};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server_progid = "Matrikon.OPC.Simulation.1";

    let sink = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(AtomicUsize::new(0));
    // 如需在另一任务中观察进度或超时收割部分结果，传入前先 clone 这两个 Arc。

    let discovered_tags = client.browse_tags(
        server_progid,
        100, // 最多发现标签数
        progress,
        sink,
        0, // 数据类型过滤（0 = 任意）
        0, // 访问权限过滤（0 = 任意）
    ).await?;

    println!("发现 {} 个标签", discovered_tags.len());
    Ok(())
}
```

### 用 DCOM 凭据连接远程服务器

访问当前登录用户够不到的远程服务器（跨域、专用服务账户）。`OpcDaClient::with_credentials` 经 `COAUTHIDENTITY` 注入凭据用于远程激活。

```rust,no_run
use opc_da_client::{AuthCredentials, OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let host = "192.168.1.10";
    let creds = AuthCredentials {
        user: "operator".to_string(),
        password: "s3cr3t".to_string(),
        domain: "PLANT".to_string(),
    };
    // user 留空 -> 当前登录用户（DCOM 默认认证）。
    let client = OpcDaClient::with_credentials(host, creds)?;

    for s in client.list_servers_with_details(host).await? {
        println!("{}  [{}]", s.prog_id, s.clsid);
    }
    Ok(())
}
```

### 服务器运行时状态

查询 `IOPCServer::GetStatus`，获取已连接服务器的状态、版本、厂商、组数与时间戳。

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server = "Matrikon.OPC.Simulation.1";

    let status = client.get_server_status(server).await?;
    println!(
        "state={:?} groups={} vendor={} version={}.{} build={}",
        status.server_state,
        status.group_count,
        status.vendor_info,
        status.major_version,
        status.minor_version,
        status.build_number,
    );
    Ok(())
}
```

### 融合订阅（推送优先，轮询兜底）

`FusionReader` 建立订阅，当反向 DCOM 回调打不通时透明切换到同步轮询。事件接收端产出 `Data(TagValue)`、`Subscribed` 或 `Fallback(错误)`——丢弃 reader 即拆除订阅（优雅退订，绝不遗留孤儿服务器组）。

```rust,no_run
use std::time::Duration;
use opc_da_client::{FusionEvent, FusionReader, FusionReaderOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = FusionReaderOptions {
        update_rate: 1000,
        fallback_timeout: Duration::from_secs(10),
        buffer: 256,
    };
    // None -> 当前登录用户；跨域访问传 Some(creds)。
    let (reader, mut rx) = FusionReader::start(
        "localhost",
        None,
        "Matrikon.OPC.Simulation.1",
        vec!["Random.Int4".to_string()],
        &opts,
    )?;

    while let Some(ev) = rx.recv().await {
        match ev {
            FusionEvent::Data(v) => println!("{} = {}", v.tag_id, v.value),
            FusionEvent::Subscribed => println!("推送订阅已激活"),
            FusionEvent::Fallback(e) => eprintln!("已回退到同步轮询：{}", e),
        }
    }
    drop(reader); // 优雅退订
    Ok(())
}
```

## 架构

库分为核心 trait 层与具体实现：

- **`OpcProvider`**：定义 OPC 操作（list/browse/read/write/subscribe/status/properties 等）的主 async trait。`list_servers_with_details` 有默认实现（退化为仅 ProgID），简单后端自动兼容。
- **`OpcDaClient`**：基于原生 `windows-rs` COM 调用的默认实现。泛型于 `ServerConnector` 以便测试，默认为 `ComConnector`。用 `OpcDaClient::default()`（本机、当前用户）或 `OpcDaClient::with_credentials(host, creds)`（远程 + 显式凭据）构造。
- **`FusionReader`**：自包含读取器，内部自建 client——推送订阅优先，自动同步轮询兜底。

深入设计见 [`architecture.md`](./architecture.md)，行为契约见 [`spec.md`](./spec.md)。

### COM 线程模型

OPC DA 依赖 Windows COM，要求按线程初始化并遵守严格的线程亲和性。`opc-da-client` 将其透明化处理：
* **专用 worker 线程**：所有 COM 操作在专用后台 worker 线程（MTA 模式初始化）上执行。
* **无需手动初始化**：调用方无需 `CoInitialize`，也不必管理 COM 生命周期。

## 平台支持

OPC DA 基于 Windows COM/DCOM，故本 crate **仅限 Windows**。

| 目标平台 | 支持 | 说明 |
|----------|------|------|
| Windows 10 / Server 2016+ | ✅ 标准 `cargo build` 产物 | 主要目标 |
| Windows 7 SP1 / Server 2008 R2 SP1 | ⚠️ 需 `compat/` polyfill + 静态 CRT | 用 `make package-win7`；polyfill 须从源码重建（本仓库未 vendor） |
| Linux / macOS | ❌ 无法编译——给出一条友好的 `compile_error!` | OPC DA 依赖 COM/DCOM |

每个目标都需在本机（DCOM 远程时为远程主机）注册 **OPC DA Core Components**。

## 致谢

本库立于前人工作之上，谨致谢意：

- **[wends155/opc-cli](https://github.com/wends155/opc-cli)**——本项目 fork 自此，其确立了 `OpcProvider` trait 抽象、专用 COM worker 线程模型与初始 TUI 客户端。
- **[Ronbb/rust_opc](https://github.com/Ronbb/rust_opc)**——`src/bindings/` 中冻结的 OPC DA / OPC Common COM 绑定由此项目生成。
- **[OPC Foundation](https://opcfoundation.org/)**——绑定所源自的 OPC Data Access 2.05a / 3.0 IDL 规范（`opcda.idl`、`OPCComn.idl`）。

## 本 Fork 的改进

相对于上游 `wends155/opc-cli`，本 fork 面向生产做了如下硬化：

- **订阅崩溃根因修复**——阻止 `VariantClear` 释放借来的 `SafeArray` 元素；消除了流式开始几秒后出现的堆损坏（`0xc0000374`）崩溃。
- **订阅自愈**——健康监控检测静默失效的 `IOPCDataCallback`（如 OPC 服务器进程被杀后），并重建订阅；服务器死亡触发 DCOM 重连（e2e 验证：杀进程后约 31s 自愈）。
- **融合读取器**——`FusionReader` 提供推送优先 / 轮询兜底的流，使被拦的反向 DCOM 回调绝不会让消费方拿到静默过期数据。其拆除在专用 OS 线程上执行（经 `DetachingClient`），丢弃远程 client 不会阻塞 tokio runtime，且优雅退订服务器组（e2e 验证），而非依赖租约到期。
- **显式 DCOM 凭据**——`AuthCredentials` + `with_credentials` 经 `COAUTHIDENTITY` 注入跨域 / 服务账户远程访问；`AuthInfo`/`ServerInfo` 桥接层现已在堆上 owned `COAUTHINFO`/`COAUTHIDENTITY`（修复 32 位 `0x800703E6` 悬垂指针崩溃），且 `Debug` 屏蔽密码，凭据永不进入日志。
- **服务器富信息**——`list_servers_with_details` 经 `GetClassDetails` 暴露 CLSID + 厂商描述。
- **panic 可观测性**——COM worker 的 `catch_unwind` 边界现捕获 panic payload/消息而非吞掉，使生产崩溃留下根因。
- **惰性分层浏览**——`browse_children` 每次树节点点击一次往返地枚举分支/叶子，而非扁平递归 dump。
- **多个并发订阅组**——修复了因静态组名导致的 `OPC_E_DUPLICATENAME`。
- **每标签数据类型**暴露在 `TagValue` 上；桌面 UI 支持内联写入。
- **生产就绪硬化**——跨目标 `cfg` 守卫 + 友好的 `compile_error!`；移除潜在的 `Clone`-on-free-on-drop 双释放；修复数组变体栈溢出；堵住 `add_items` 失败路径上的服务器端 OPC 组泄漏；新增 `hr_code()` HRESULT 访问器，使消费方与 `windows-rs` 解耦。

## 第三方许可

`src/bindings/` 中的 OPC DA / OPC Common COM 绑定衍生自
[`Ronbb/rust_opc`](https://github.com/Ronbb/rust_opc)，由 `windows-bindgen`
生成。Copyright © 2025 Wang Ruobiao，按 MIT 许可分发。完整的第三方声明
（含依赖许可）位于工作区根目录的 `THIRD_PARTY_LICENSES.md`。

## 许可

本项目基于 MIT 许可授权。
