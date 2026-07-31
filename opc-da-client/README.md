# opc-da-client

[![Crates.io](https://img.shields.io/crates/v/opc-da-client.svg)](https://crates.io/crates/opc-da-client)
[![Docs.rs](https://docs.rs/opc-da-client/badge.svg)](https://docs.rs/opc-da-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Backend-agnostic OPC DA client library for Rust — async, trait-based, with transparent COM management.

## Features

- **Async/Await API**: Built for modern asynchronous Rust using `tokio` and `async-trait`.
- **Trait-Based Abstraction**: The `OpcProvider` trait allows for easy mocking and backend swapping.
- **Transparent COM Management**: Handles COM initialization (`CoInitializeEx`) and apartment thread affinity automatically in the background.
- **Full OPC DA Coverage**: Browse, read, write, subscribe (IOPCDataCallback), shutdown notifications (IOPCShutdown), server status, item properties, MaxAge read, VQT write, batch operations, and DA 3.0 interfaces.
- **Remote DCOM**: Connect to OPC DA servers on remote hosts via DCOM (`CoCreateInstanceEx`).
- **Connection Resilience**: Connection pooling with stale-proxy eviction, exponential-backoff reconnect, explicit disconnect/reconnect API.
- **Windows COM/DCOM Support**: Native OPC DA backend via `windows-rs` — no external OPC crates needed.
- **Robust Error Handling**: Leverages `thiserror` for the `OpcError` domain type and `friendly_com_hint()` for human-readable HRESULT explanations.
- **Test-Friendly**: Built-in `MockOpcProvider` via the `test-support` feature; 19-test end-to-end suite against real Matrikon/Kepware servers (`e2e` feature).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
opc-da-client = "0.3"
```

## Prerequisites

- **Operating System**: Windows (COM/DCOM is a Windows-only technology).
- **OPC DA Core Components**: Ensure the OPC DA Core Components are installed and registered on your system.
- **DCOM Configuration**: If connecting to remote servers, appropriate DCOM permissions must be configured.

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

## Architecture

The library is split into a core trait layer and concrete implementations:

- **`OpcProvider`**: The primary async trait defining OPC operations (list, browse, read, write).
- **`OpcDaClient`**: The default implementation using native `windows-rs` COM calls. Generic over `ServerConnector` for testability; defaults to `ComConnector`.

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
- **Panic observability** — the COM worker's `catch_unwind` boundary now captures the panic payload/message instead of swallowing it, so production crashes leave a root cause.
- **Lazy hierarchical browse** — `browse_children` does one-round-trip branch/leaf enumeration per tree-node click, instead of a flat recursive dump.
- **Multiple concurrent subscription groups** — fixed `OPC_E_DUPLICATENAME` from a static group name.
- **Per-tag data type** surfaced on `TagValue`; inline write from the desktop UI.

## Third-Party Licenses

The OPC DA / OPC Common COM bindings in `src/bindings/` are derived from
[`Ronbb/rust_opc`](https://github.com/Ronbb/rust_opc), generated with
`windows-bindgen`. Copyright © 2025 Wang Ruobiao, distributed under the MIT
License. The full third-party notices (including dependency licenses) live in
`THIRD_PARTY_LICENSES.md` at the workspace root.

## License

This project is licensed under the MIT License.
