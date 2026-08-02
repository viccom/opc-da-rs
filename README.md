# OPC DA Client CLI

> **Language**: [English](README.md) | [简体中文](README.zh-CN.md)

A modern, asynchronous TUI (Terminal User Interface) client for browsing, reading, and writing OPC DA (Data Access) tags on Windows.

## 🧩 Components

This repository is a Cargo workspace with three crates:

| Crate | Description |
| :--- | :--- |
| **[`opc-cli`](./opc-cli/)** | The interactive TUI application (`ratatui` + `crossterm`). The binary this README focuses on. |
| **[`opc-da-client`](./opc-da-client/)** | The async OPC DA library — the `OpcProvider` trait + native `windows-rs` COM backend, used by both apps. ([README](./opc-da-client/README.md)) |
| **[`opc-da-desktop`](./opc-da-desktop/)** | A Tauri 2 desktop GUI (React + TypeScript) for browsing, subscribing, and writing tags. ([README](./opc-da-desktop/README.md)) |

Further reading: high-level map in **[ARCHITECTURE_DIAGRAM.md](./ARCHITECTURE_DIAGRAM.md)**, library deep-dive in **[opc-da-client/architecture.md](./opc-da-client/architecture.md)**, and the sync/async/subscription + DCOM walkthrough in **[DCOM_GUIDE.md](./DCOM_GUIDE.md)**.

## 🏗️ Architecture

The project is structured as a Cargo workspace (see above):

- **`opc-cli`**: The interactive TUI application built with `ratatui` + `crossterm`.
- **`opc-da-client`**: A native Windows COM library (using `windows-rs`) that abstracts OPC DA communication through an async trait (`OpcProvider`). Generic over `ServerConnector` for easy mocking.

See **[opc-da-client/architecture.md](./opc-da-client/architecture.md)** for the full design, state machine, and data flow diagrams.

## ✨ Features

- **Server Discovery**: Enumerate OPC DA servers on local or remote hosts (DCOM), with optional CLSID + vendor description (`list_servers_with_details`).
- **Explicit DCOM Credentials**: Authenticate to remote servers with a dedicated user/password/domain — for cross-domain or service-account access.
- **Hierarchical Browsing**: Recursive exploration of complex server namespaces with partial-result harvesting on timeout, data-type/access-rights filtering, and lazy one-level browse (`browse_children`).
- **Real-time Subscription**: Event-driven data-change notifications via `IOPCDataCallback` (true push, not polling), with self-healing on a silently-dead callback. The TUI also supports 1-second auto-refresh polling as a fallback.
- **Fusion Reader**: A push-preferred / polling-fallback stream (`FusionReader`) so a blocked reverse-DCOM callback can never leave consumers with stale data — used by the desktop app.
- **Tag Read/Write**: Synchronous read, MaxAge read (`IOPCSyncIO2`), VQT write (value + quality + timestamp), and batch operations.
- **Diagnostics**: Server status (`IOPCServer::GetStatus`), item properties (EU/data type/access rights), localized error strings, server-shutdown notifications.
- **Connection Resilience**: Connection pooling with stale-proxy eviction, exponential-backoff reconnect, explicit disconnect/reconnect API.
- **Search & Filter**: Substring search with `Tab`/`Shift+Tab` cycling through matches.
- **Rich Error Hints**: Human-readable explanations for cryptic Windows COM/DCOM HRESULT codes.
- **Transparent COM Management**: COM initialization and apartment thread affinity handled automatically by a dedicated background worker thread.
- **Mockable Backend**: Unit-test the TUI on any OS without a live OPC server.

## 🚀 Getting Started

### Prerequisites

- **Windows OS**: This application uses Windows COM/DCOM.
- **OPC Core Components**: Must be installed on the system to resolve OPC ProgIDs.
- **Rust 1.93+**: Edition 2024.

### Build & Run

```powershell
# Run the TUI
cargo run --bin opc-cli

# Run the full verification gate (format → lint → test)
pwsh -File scripts/verify.ps1
```

## ⌨️ Controls

| Key | Action | Screen |
| :--- | :--- | :--- |
| `Enter` | Navigate forward / Confirm input | All |
| `Esc` | Navigate back | All |
| `Space` | Toggle tag selection | Tag List |
| `s` | Enter search/filter mode | Tag List |
| `Tab` / `Shift+Tab` | Cycle through search matches | Tag List (search) |
| `w` | Enter write mode for selected tag | Tag Values |
| `↑` / `↓` | Navigate lists | All lists |
| `PgUp` / `PgDn` | Page through lists (20 items) | All lists |
| `q` / `Q` | Quit application | Home |

## 📦 Packaging & Deployment

The repository supports two release packaging models:

### 1. Modern Release (Windows 10+ / Server 2016+)

```powershell
make package
# OR
pwsh -File scripts/package.ps1 package
```
Output: `dist/opc-cli-x64/` and `dist/opc-cli-x64.zip`

### 2. Legacy Release (Windows 7 SP1 / Server 2008 R2 SP1)

For deployment to offline, air-gapped industrial environments running Windows 7 / Server 2008 R2 (NT 6.1):

```powershell
make package-win7
# OR
pwsh -File scripts/package.ps1 package-win7
```
Output: `dist/opc-cli-win7-x64/` and `dist/opc-cli-win7-x64.zip`

**Legacy Bundle Contents:**
- `opc-cli.exe`: PE-patched executable linked with static CRT (`+crt-static`). Replaces missing `GetSystemTimePreciseAsFileTime` imports with native `GetSystemTimeAsFileTime`.
- `api-ms-win-core-synch-l1-2-0.dll`: `#![no_std]` polyfill for `WaitOnAddress` and `Sleep` re-export.
- `api-ms-win-core-winrt-error-l1-1-0.dll`: `#![no_std]` no-op stubs for WinRT error APIs.
- `bcryptprimitives.dll`: `#![no_std]` polyfill routing `ProcessPrng` to `RtlGenRandom` (`advapi32.dll`).
- `redist/`: Included OPC Core Components redistributable MSI (if placed in `vendor/redist/`).

Simply copy the extracted `dist/opc-cli-win7-x64/` folder to a USB drive and run on the target machine without installing Visual C++ redistributables or Windows updates.

## 🙏 Acknowledgments

- [**rust_opc**](https://github.com/Ronbb/rust_opc) by Wang Ruobiao — original OPC DA Rust bindings and COM interface generation pipeline.
- [**OPC Foundation**](https://opcfoundation.org/) — OPC Data Access specification and IDL interface definitions.
- [**windows-rs**](https://github.com/microsoft/windows-rs) by Microsoft — Windows API bindings for Rust.
- [**ratatui**](https://github.com/ratatui/ratatui) — terminal user interface framework.

## 📄 License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.
