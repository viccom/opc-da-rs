# OPC DA Desktop

> **语言**：[English](README.md) | 简体中文

基于 Tauri 2.0 的桌面 GUI，用于在 Windows 上浏览、订阅与可视化 OPC DA 标签。

> **平台**：仅限 Windows（依赖使用 COM/DCOM 的 `opc-da-client`）。
> 该库在 Linux/macOS 上编译为空 stub，使 `cargo check --workspace` 能在跨平台 CI 上通过，但实际应用需 Windows。

## 架构

```
opc-da-desktop/
├── Cargo.toml         # 经 path 依赖 opc-da-client
├── tauri.conf.json    # 窗口 / bundle 配置
├── build.rs           # tauri-build
├── src/               # Rust 后端（Tauri commands）
│   ├── main.rs        # Windows 入口；非 Windows stub
│   ├── lib.rs
│   ├── app.rs         # tauri::Builder + handler 注册
│   ├── state.rs       # AppState（client + 凭据 + 订阅 + fusion readers）
│   ├── error.rs       # DesktopError / DesktopResult
│   ├── commands/      # #[tauri::command] handlers
│   │   ├── mod.rs
│   │   ├── servers.rs       # list_servers / connect / disconnect / set_host / get_server_status
│   │   ├── browse.rs        # browse_tags / browse_children（模态树）
│   │   ├── read.rs          # read_tag_values
│   │   ├── write.rs         # write_tag_value
│   │   └── subscription.rs  # subscribe_tags / unsubscribe_tags（+ fusion 变体）
│   └── ipc/
│       ├── mod.rs                 # TagUpdate / FusionEventDto 载荷类型
│       ├── subscription_runner.rs # rx → Channel<TagUpdate>
│       └── fusion_runner.rs       # rx → Channel<FusionEventDto>
└── ui/                # React 18 + TypeScript 前端
    ├── package.json
    ├── vite.config.ts
    ├── index.html
    └── src/
        ├── main.tsx
        ├── App.tsx          # 主布局
        ├── api/tauri.ts     # invoke / Channel 包装
        ├── stores/          # zustand 状态切片
        └── components/      # ServerPanel / GroupEditor / TagTable / …
```

## 功能特性

1. **服务器列表** — 枚举主机上可达的 OPC DA 服务器，附带 CLSID + 类型（`list_servers_with_details`）。
2. **带凭据连接** — 绑定 ProgID；可选 user / password / domain 用于跨域 DCOM（`set_host` → `OpcDaClient::with_credentials`）。user 留空 = 当前登录用户。
3. **服务器运行时状态** — 状态 / 厂商 / 版本 / 启动 / 当前 / 最后更新时间 / 组数 / 带宽（`get_server_status`，`IOPCServer::GetStatus`）。
4. **订阅组** — 名称 / 更新速率 / 死区 / 客户端句柄，二选一模式：
   - **订阅** — 经 `IOPCDataCallback` 推送。
   - **融合** — 推送优先，反向 DCOM 回调被拦时自动回退同步轮询（`FusionReader`）。
5. **标签浏览器模态** — 节点树 + 叶子窗格用于添加标签（`browse_children`）。
6. **实时表格** — 经 Tauri 2 `Channel<TagUpdate>` / `Channel<FusionEventDto>` 的高频更新，支持名称过滤。
7. **干净断开** — 解绑 ProgID 前停止所有订阅与 fusion reader（断开后无残留刷新）。

## 构建

```sh
# 先构建前端（Vite 生产产物落入 ui/dist/）
cd ui && npm install && npm run build && cd ..

# 再构建 Tauri bundle
cargo build --release
```

开发模式：

```sh
cd ui && npm run dev      # 终端 1：Vite dev server，端口 :5173
cargo tauri dev           # 终端 2：Tauri 外壳，带热重载
```

## 日志

`logs/opc-da-desktop.log` — 每日滚动，格式同 `opc-cli`。
用 `RUST_LOG` 配置（如 `RUST_LOG=debug,opc_da_client=trace`）。

## 许可

MIT — 见工作区 `LICENSE`。
