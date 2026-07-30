# OPC DA Desktop

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
│   ├── state.rs       # AppState (Arc<OpcDaClient> + subscription map)
│   ├── error.rs       # DesktopError / DesktopResult
│   ├── commands/      # #[tauri::command] handlers
│   │   ├── mod.rs
│   │   ├── servers.rs       # list_servers / connect / disconnect
│   │   ├── browse.rs        # browse_tags (modal tree)
│   │   ├── read.rs          # read_tag_values
│   │   ├── write.rs         # write_tag_value
│   │   └── subscription.rs  # subscribe_tags / unsubscribe_tags
│   └── ipc/
│       ├── mod.rs           # TagUpdate payload type
│       └── subscription_runner.rs  # rx → Channel<TagUpdate>
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

## Features (v0.1.0)

1. **Server list** — enumerate OPC DA servers reachable on a host.
2. **Connect** — bind a ProgID to a live `OpcDaClient`.
3. **Subscription group** — name / update rate / deadband / client handle.
4. **Tag browser modal** — node tree + leaves pane for adding tags.
5. **Real-time table** — high-frequency updates via Tauri 2 `Channel<TagUpdate>`,
   with name filtering.

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