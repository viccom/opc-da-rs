# OPC DA Client & Server (Rust)

> **Language**: [English](README.md) | [简体中文](README.zh-CN.md)

A modern Rust workspace for OPC DA (Data Access) on Windows — async **clients** (TUI + desktop GUI) for browsing / reading / writing / subscribing to tags, plus a native **OPC DA server** library and a Matrikon-style **simulation server** for client testing and protocol-gateway development (Modbus / S7 / UA bridge).

## 🧩 Components

This repository is a Cargo workspace with six crates:

| Crate | Side | Description |
| :--- | :--- | :--- |
| **[`opc-cli`](./opc-cli/)** | client | Interactive TUI application (`ratatui` + `crossterm`). |
| **[`opc-da-desktop`](./opc-da-desktop/)** | client | Tauri 2 desktop GUI (React + TypeScript) for browsing / subscribing / writing. ([README](./opc-da-desktop/README.md)) |
| **[`opc-da-client`](./opc-da-client/)** | client | Async OPC DA library — `OpcProvider` trait + native `windows-rs` COM backend, shared by both clients. ([README](./opc-da-client/README.md)) |
| **[`opc-da-server`](./opc-da-server/)** | server | OPC DA Server COM library — `IOPCServer` / `Group` / `ItemMgt` / `SyncIO` / `AsyncIO2` / `Browse` / `ItemProperties` + global push scheduler + GIT cross-apartment callback. |
| **[`opc-da-server-sim`](./opc-da-server-sim/)** | server | Simulation server (drop-in for `Matrikon.OPC.Simulation`), x64 + x86. ([README](./opc-da-server-sim/README.md)) |
| **[`opc-da-client-test`](./opc-da-client-test/)** | test | End-to-end test harness driving `opc-da-client` against `opc-da-server` / `opc-da-server-sim`. |

Further reading: high-level map in **[ARCHITECTURE_DIAGRAM.md](./ARCHITECTURE_DIAGRAM.md)**, client library deep-dive in **[opc-da-client/architecture.md](./opc-da-client/architecture.md)**, simulation server guide in **[opc-da-server-sim/README.md](./opc-da-server-sim/README.md)**, and the sync/async/subscription + DCOM walkthrough in **[DCOM_GUIDE.md](./DCOM_GUIDE.md)**.

## 🏗️ Architecture

The project is structured as a Cargo workspace spanning both sides of OPC DA:

- **Clients** (`opc-cli`, `opc-da-desktop`) sit on top of the **`opc-da-client`** library — a native Windows COM library (`windows-rs`) that abstracts OPC DA through an async trait (`OpcProvider`), generic over `ServerConnector` for easy mocking.
- **Servers** (`opc-da-server-sim`) sit on top of the **`opc-da-server`** library — a native OPC DA Server COM implementation (`IOPCServer` / `Group` / `SyncIO` / `AsyncIO2` / `Browse` …) with a global push scheduler and Global Interface Table (GIT) cross-apartment callback, so STA clients (PsOPCClient, Prosys) receive `OnDataChange` without `RPC_E_WRONG_THREAD`.

See **[opc-da-client/architecture.md](./opc-da-client/architecture.md)** for the full client design, and **[opc-da-server-sim/README.md](./opc-da-server-sim/README.md)** for the server-side wiring.

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

## 🖥️ OPC DA Server (`opc-da-server-sim`)

`opc-da-server-sim` is a standalone OPC DA Simulation Server mirroring the `Matrikon.OPC.Simulation` tag set — use it to develop and test clients without a real PLC, or as a template for building your own protocol-gateway servers (Modbus / S7 / UA bridge). It is a thin wrapper over the `opc-da-server` library, demonstrating the full "plug in a custom `DataSource` → register → serve" flow.

**Register & run** (run from an elevated prompt):

```powershell
# 64-bit build — for 64-bit OPC clients
target\release\opc-da-server-sim.exe /RegServer

# 32-bit build — REQUIRED for 32-bit desktop clients (PsOPCClient, Prosys OPC Client)
cargo build --release -p opc-da-server-sim --target i686-pc-windows-msvc
target\i686-pc-windows-msvc\release\opc-da-server-sim.exe /RegServer
```

Then connect any standard OPC DA client to ProgID **`opc-da-rs.Sim.1`** (hierarchical namespace, e.g. `Random.Int4.0`, `BucketBrigade.Int4.0`, `_System.Time`). Scale the tag set via `opc-da-server-sim.ini` (`count = N`, up to 100 000 → ~800k tags).

> ⚠ **Bitness**: a 32-bit client connecting to a 64-bit server may crash on cross-bitness marshaling when `System32\OPCProxy.dll` (64-bit) is missing — match the server bitness to your client, or install the 64-bit OPC Core Components. Full details in [opc-da-server-sim/README.md](./opc-da-server-sim/README.md).

CI builds **both** bitnesses and publishes `opc-da-server-sim-x64.exe` + `opc-da-server-sim-x86.exe` (+ `opc-da-server-x64.exe`) to the GitHub Release on every `v*` tag.

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
