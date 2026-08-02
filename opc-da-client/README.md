# opc-da-client

[![Crates.io](https://img.shields.io/crates/v/opc-da-client.svg)](https://crates.io/crates/opc-da-client)
[![Docs.rs](https://docs.rs/opc-da-client/badge.svg)](https://docs.rs/opc-da-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **Language**: [English](README.md) | [简体中文](README.zh-CN.md)
>
> **Mirror**: also mirrored on [Gitea](https://git.metme.top/viccom/opc-da-lib). Primary development on [GitHub](https://github.com/viccom/opc-da-rs).

Backend-agnostic OPC DA client library for Rust — async, trait-based, with transparent COM management.

## Features

- **Async/Await API**: Built for modern asynchronous Rust using `tokio` and `async-trait`.
- **Trait-Based Abstraction**: The `OpcProvider` trait allows for easy mocking and backend swapping.
- **Transparent COM Management**: Handles COM initialization (`CoInitializeEx`) and apartment thread affinity automatically in the background.
- **Full OPC DA Coverage**: Browse, read, write, subscribe (`IOPCDataCallback`), shutdown notifications (`IOPCShutdown`), server status, item properties, MaxAge read, VQT write, batch operations, and DA 3.0 interfaces.
- **Remote DCOM**: Connect to OPC DA servers on remote hosts via DCOM (`CoCreateInstanceEx`).
- **Explicit DCOM Credentials**: Authenticate to remote servers with a dedicated user/password/domain (`AuthCredentials` + `OpcDaClient::with_credentials`) — for cross-domain or service-account access the logged-in user cannot reach. Empty user falls back to the current logged-in user (DCOM default auth).
- **Fusion Reader**: `FusionReader` prefers a true push subscription and automatically falls back to synchronous polling when the reverse-DCOM callback cannot get through (blocked inbound DCOM, restrictive firewall) — one stream, never silently dead.
- **Server Details & Status**: `list_servers_with_details` enriches the server list with CLSID + vendor description (`IOPCServerList::GetClassDetails`); `get_server_status` exposes runtime state, version, vendor, group count, and timestamps (`IOPCServer::GetStatus`).
- **Connection Resilience**: Connection pooling with stale-proxy eviction, exponential-backoff reconnect, explicit disconnect/reconnect API.
- **Windows COM/DCOM Support**: Native OPC DA backend via `windows-rs` — no external OPC crates needed.
- **Robust Error Handling**: Leverages `thiserror` for the `OpcError` domain type and `friendly_com_hint()` for human-readable HRESULT explanations.
- **Test-Friendly**: Built-in `MockOpcProvider` via the `test-support` feature; end-to-end suite against real Matrikon/Kepware servers (`e2e` feature).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
opc-da-client = "0.3"
```

> **Note — unreleased features.** DCOM credentials, `FusionReader`, and
> `list_servers_with_details` live on the `main` branch and are **not yet on
> crates.io** (latest published is `0.3.1`). To use them today, depend on the
> Git revision:
>
> ```toml
> opc-da-client = { git = "https://github.com/viccom/opc-da-rs" }
> ```

## Prerequisites

- **Operating System**: Windows (COM/DCOM is a Windows-only technology).
- **OPC DA Core Components**: Ensure the OPC DA Core Components are installed and registered on your system.
- **DCOM Configuration**: If connecting to remote servers, appropriate DCOM permissions must be configured. See the workspace [`DCOM_GUIDE.md`](../DCOM_GUIDE.md) for the sync/async/subscription direction and auth differences, plus local-vs-remote setup and troubleshooting.

## Usage Examples

### Connecting & Listing Servers

Enumerate available OPC DA servers on a local or remote host.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();

    let servers = client.list_servers("localhost").await?;
    println!("Available Servers:");
    for server in servers {
        println!("  - {}", server);
    }
    Ok(())
}
```

### Listing Servers with Details

`list_servers_with_details` enriches each entry with its CLSID and a vendor
user-type description, via `IOPCServerList::GetClassDetails`.

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

### Reading Tags

Connect to a specific server and read current values for a set of tags.

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

### Writing a Value

Write a typed value to a single OPC tag.

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
        println!("✓ Write succeeded");
    } else {
        println!("✗ Write failed: {}", result.error.as_deref().unwrap_or("Unknown error"));
    }
    Ok(())
}
```

### Browsing the Address Space

Recursively discover available tags on an OPC server.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};
use std::sync::{Arc, Mutex, atomic::AtomicUsize};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server_progid = "Matrikon.OPC.Simulation.1";

    let sink = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(AtomicUsize::new(0));
    // Clone these Arcs before passing if you need to monitor progress
    // or harvest partial results from another task on timeout.

    let discovered_tags = client.browse_tags(
        server_progid,
        100, // Max tags to discover
        progress,
        sink,
        0, // data_type filter (0 = any)
        0, // access_rights filter (0 = any)
    ).await?;

    println!("Found {} tags", discovered_tags.len());
    Ok(())
}
```

### Remote Server with DCOM Credentials

Reach a remote server the current logged-in user cannot access (cross-domain,
dedicated service account). `OpcDaClient::with_credentials` injects the
credentials via `COAUTHIDENTITY` for remote activation.

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
    // user empty -> current logged-in user (DCOM default auth).
    let client = OpcDaClient::with_credentials(host, creds)?;

    for s in client.list_servers_with_details(host).await? {
        println!("{}  [{}]", s.prog_id, s.clsid);
    }
    Ok(())
}
```

### Server Runtime Status

Query `IOPCServer::GetStatus` for the connected server's state, version, vendor,
group count, and timestamps.

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

### Fusion Subscription (push preferred, polling fallback)

`FusionReader` opens a subscription and transparently switches to synchronous
polling if the reverse-DCOM callback cannot get through. The event receiver
yields `Data(TagValue)`, `Subscribed`, or `Fallback(error)` — drop the reader to
tear the subscription down (it unsubscribes gracefully, never leaving an orphan
server group).

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
    // None -> current logged-in user; Some(creds) for cross-domain access.
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
            FusionEvent::Subscribed => println!("push subscription active"),
            FusionEvent::Fallback(e) => eprintln!("sync polling fallback: {}", e),
        }
    }
    drop(reader); // graceful unsubscribe
    Ok(())
}
```

## Architecture

The library is split into a core trait layer and concrete implementations:

- **`OpcProvider`**: The primary async trait defining OPC operations (list, browse, read, write, subscribe, status, properties, …). `list_servers_with_details` has a default impl that degrades to ProgID-only, so simple backends stay compatible.
- **`OpcDaClient`**: The default implementation using native `windows-rs` COM calls. Generic over `ServerConnector` for testability; defaults to `ComConnector`. Construct with `OpcDaClient::default()` (localhost, current user) or `OpcDaClient::with_credentials(host, creds)` (remote + explicit credentials).
- **`FusionReader`**: A self-contained reader that spins up its own client(s) internally — preferred push subscription with automatic synchronous-polling fallback.

See [`architecture.md`](./architecture.md) for in-depth design details and [`spec.md`](./spec.md) for behavioral contracts.

### COM Threading Model

OPC DA relies on Windows COM, which requires per-thread initialization and strict thread affinity. `opc-da-client` handles this transparently:
* **Dedicated Worker Thread**: All COM operations are executed on a dedicated background worker thread initialized in Multi-Threaded Apartment (MTA) mode.
* **No Manual Init**: You do not need to call `CoInitialize` or manage COM lifecycles in your calling application.

## Platform Support

OPC DA is built on Windows COM/DCOM, so this crate is **Windows-only**.

| Target | Support | Notes |
|--------|---------|-------|
| Windows 10 / Server 2016+ | ✅ Standard `cargo build` output | Primary target |
| Windows 7 SP1 / Server 2008 R2 SP1 | ⚠️ Requires `compat/` polyfill + static CRT | Use `make package-win7`; the polyfill must be rebuilt from source (not currently vendored in this repo) |
| Linux / macOS | ❌ Will not compile — yields a single friendly `compile_error!` | OPC DA depends on COM/DCOM |

Every target needs the **OPC DA Core Components** registered locally (or on the remote host for DCOM).

## Acknowledgements

This library stands on prior work — grateful acknowledgement:

- **[wends155/opc-cli](https://github.com/wends155/opc-cli)** — the original project this was forked from, which established the `OpcProvider` trait abstraction, the dedicated COM worker-thread model, and the initial TUI client.
- **[Ronbb/rust_opc](https://github.com/Ronbb/rust_opc)** — the frozen OPC DA / OPC Common COM bindings in `src/bindings/` were generated from this project.
- **[OPC Foundation](https://opcfoundation.org/)** — the OPC Data Access 2.05a / 3.0 IDL specifications (`opcda.idl`, `OPCComn.idl`) the bindings are derived from.

## Improvements in this Fork

Relative to upstream `wends155/opc-cli`, this fork hardens the library for production:

- **Subscription crash root-cause fix** — stopped `VariantClear` from freeing borrowed `SafeArray` elements; eliminated the heap-corruption (`0xc0000374`) crash that appeared a few seconds into streaming.
- **Subscription self-healing** — a health monitor detects a silently-dead `IOPCDataCallback` (e.g. after the OPC server process is killed) and rebuilds the subscription; a dead server triggers DCOM reconnect (e2e-verified: ~31s recovery after kill).
- **Fusion reader** — `FusionReader` delivers a push-preferred / polling-fallback stream so a blocked reverse-DCOM callback can never leave the consumer with silently-stale data. Its teardown runs on a dedicated OS thread (via `DetachingClient`) so dropping a remote client cannot block the tokio runtime, and it unsubscribes the server group gracefully (e2e-verified) instead of relying on lease expiry.
- **Explicit DCOM credentials** — `AuthCredentials` + `with_credentials` inject `COAUTHIDENTITY` for cross-domain / service-account remote access; the `AuthInfo`/`ServerInfo` bridge now owns its `COAUTHINFO`/`COAUTHIDENTITY` on the heap (fixed the 32-bit `0x800703E6` dangling-pointer crash), and `Debug` masks passwords so credentials never reach logs.
- **Server details** — `list_servers_with_details` surfaces CLSID + vendor description via `GetClassDetails`.
- **Panic observability** — the COM worker's `catch_unwind` boundary now captures the panic payload/message instead of swallowing it, so production crashes leave a root cause.
- **Lazy hierarchical browse** — `browse_children` does one-round-trip branch/leaf enumeration per tree-node click, instead of a flat recursive dump.
- **Multiple concurrent subscription groups** — fixed `OPC_E_DUPLICATENAME` from a static group name.
- **Per-tag data type** surfaced on `TagValue`; inline write from the desktop UI.
- **Production-readiness hardening** — cross-target `cfg` guard with a friendly `compile_error!`, removed a latent `Clone`-on-free-on-drop double-free, fixed an array-variant stack overflow, plugged a server-side OPC group leak on `add_items` failure paths, and added an `hr_code()` HRESULT accessor that decouples consumers from `windows-rs`.

## Third-Party Licenses

The OPC DA / OPC Common COM bindings in `src/bindings/` are derived from
[`Ronbb/rust_opc`](https://github.com/Ronbb/rust_opc), generated with
`windows-bindgen`. Copyright © 2025 Wang Ruobiao, distributed under the MIT
License. The full third-party notices (including dependency licenses) live in
`THIRD_PARTY_LICENSES.md` at the workspace root.

## License

This project is licensed under the MIT License.
