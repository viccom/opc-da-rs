//! Minimal remote OPC DA server enumeration diagnostic.
//!
//! Connects to the given host via DCOM (`IOPCServerList`) and prints the available
//! OPC DA server ProgIDs, surfacing the raw HRESULT on failure (DCOM permission /
//! firewall / bitness issues show up here).
//!
//! # Usage
//!
//! 64-bit (default toolchain):
//! ```sh
//! cargo run -p opc-da-client --example remote_list -- 192.168.199.155
//! ```
//!
//! 32-bit (matches legacy 32-bit OPC clients like Takebishi):
//! ```sh
//! rustup target add i686-pc-windows-msvc
//! cargo run -p opc-da-client --target i686-pc-windows-msvc --example remote_list -- 192.168.199.155
//! ```

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, friendly_com_hint};

#[tokio::main]
async fn main() {
    let host = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "localhost".to_string());

    let bits = if cfg!(target_pointer_width = "32") {
        32
    } else {
        64
    };
    eprintln!(">>> Enumerating OPC DA servers on '{host}' ({bits}-bit client)");

    let client = match OpcDaClient::new(ComConnector::new(&host)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("OpcDaClient init FAILED: {e}");
            return;
        }
    };

    match client.list_servers(&host).await {
        Ok(servers) => {
            println!("OK: found {} server(s) on {host}", servers.len());
            for s in &servers {
                println!("  - {s}");
            }
        }
        Err(e) => {
            eprintln!("list_servers FAILED on {host}: {e}");
            if let Some(hint) = friendly_com_hint(&e) {
                eprintln!("HINT: {hint}");
            }
        }
    }
}
