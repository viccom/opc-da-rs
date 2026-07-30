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

See [architecture.md](https://github.com/wends155/opc-cli/blob/main/opc-da-client/architecture.md) for in-depth design details and [spec.md](https://github.com/wends155/opc-cli/blob/main/opc-da-client/spec.md) for behavioral contracts.

### COM Threading Model

OPC DA relies on Windows COM, which requires per-thread initialization and strict thread affinity. `opc-da-client` handles this transparently:
* **Dedicated Worker Thread**: All COM operations are executed on a dedicated background worker thread initialized in Multi-Threaded Apartment (MTA) mode.
* **No Manual Init**: You do not need to call `CoInitialize` or manage COM lifecycles in your calling application.

## License

This project is licensed under the MIT License.
