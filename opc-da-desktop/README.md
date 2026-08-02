# OPC DA Desktop

> **Language**: [English](README.md) | [简体中文](README.zh-CN.md)

Tauri 2.0 desktop GUI for browsing, subscribing, and visualizing OPC DA tags on Windows.

> **Platform**: Windows only (depends on `opc-da-client` which uses COM/DCOM).
> The library compiles on Linux/macOS as an empty stub so `cargo check --workspace`
> can pass on cross-platform CI, but the actual application requires Windows.

## Architecture

```
opc-da-desktop/
├── Cargo.toml         # depends on opc-da-client via path
├── tauri.conf.json    # window / bundle config
├── build.rs           # tauri-build
├── src/               # Rust backend (Tauri commands)
│   ├── main.rs        # Windows entry; non-Windows stub
│   ├── lib.rs
│   ├── app.rs         # tauri::Builder + handler registration
│   ├── state.rs       # AppState (client + credentials + subscriptions + fusion readers)
│   ├── error.rs       # DesktopError / DesktopResult
│   ├── commands/      # #[tauri::command] handlers
│   │   ├── mod.rs
│   │   ├── servers.rs       # list_servers / connect / disconnect / set_host / get_server_status
│   │   ├── browse.rs        # browse_tags / browse_children (modal tree)
│   │   ├── read.rs          # read_tag_values
│   │   ├── write.rs         # write_tag_value
│   │   └── subscription.rs  # subscribe_tags / unsubscribe_tags (+ fusion variants)
│   └── ipc/
│       ├── mod.rs                 # TagUpdate / FusionEventDto payload types
│       ├── subscription_runner.rs # rx → Channel<TagUpdate>
│       └── fusion_runner.rs       # rx → Channel<FusionEventDto>
└── ui/                # React 18 + TypeScript frontend
    ├── package.json
    ├── vite.config.ts
    ├── index.html
    └── src/
        ├── main.tsx
        ├── App.tsx          # main layout
        ├── api/tauri.ts     # invoke / Channel wrappers
        ├── stores/          # zustand state slices
        └── components/      # ServerPanel / GroupEditor / TagTable / …
```

## Features

1. **Server list** — enumerate OPC DA servers reachable on a host, enriched with CLSID + type (`list_servers_with_details`).
2. **Connect with credentials** — bind a ProgID; optional user / password / domain for cross-domain DCOM (`set_host` → `OpcDaClient::with_credentials`). Empty user = current logged-in user.
3. **Server runtime status** — state / vendor / version / start / current / last-update time / group count / bandwidth (`get_server_status`, `IOPCServer::GetStatus`).
4. **Subscription group** — name / update rate / deadband / client handle, in one of two modes:
   - **Subscription** — push via `IOPCDataCallback`.
   - **Fusion** — push preferred, automatic synchronous-polling fallback (`FusionReader`) when the reverse-DCOM callback is blocked.
5. **Tag browser modal** — node tree + leaves pane for adding tags (`browse_children`).
6. **Real-time table** — high-frequency updates via Tauri 2 `Channel<TagUpdate>` / `Channel<FusionEventDto>`, with name filtering.
7. **Clean disconnect** — stops every subscription and fusion reader before unbinding the ProgID (no stale refresh after disconnect).

## Build

```sh
# Frontend first (Vite production build lands in ui/dist/)
cd ui && npm install && npm run build && cd ..

# Then the Tauri bundle
cargo build --release
```

For development:

```sh
cd ui && npm run dev      # terminal 1: Vite dev server on :5173
cargo tauri dev           # terminal 2: Tauri shell with hot reload
```

## Logging

`logs/opc-da-desktop.log` — daily-rolling, same format as `opc-cli`.
Configure with `RUST_LOG` (e.g. `RUST_LOG=debug,opc_da_client=trace`).

## License

MIT — see workspace `LICENSE`.