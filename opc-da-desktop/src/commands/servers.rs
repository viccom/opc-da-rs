//! Server-side IPC handlers: list / pick a ProgID.
//!
//! These back the WebView's left rail (your functions 1, 2, 3).
//!
//! Note: `OpcDaClient` does not have an explicit `connect()` step — it
//! is stateless across ProgIDs and resolves the connection lazily on
//! the first operation that names a `server`. We track the active
//! `ProgID` in [`AppState::set_prog_id`] so subsequent commands can
//! pass it back to the trait's `server: &str` parameter.

use serde::{Deserialize, Serialize};
use tauri::State;

use opc_da_client::OpcProvider;

use crate::error::DesktopResult;
use crate::state::AppState;

/// One server entry returned by `list_servers`.
///
/// The `opc-da-client` 0.3.0 API returns `Vec<String>` (ProgIDs only);
/// vendor/CLSID enrichment is left for a future API extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// ProgID / programmatic identifier (the canonical key).
    pub prog_id: String,
}

/// Enumerate OPC DA servers reachable on the given host.
///
/// Independent of any "connected" state — talks directly to the OPC
/// ServerList enumerator on `host`.
#[tauri::command]
pub async fn list_servers(
    state: State<'_, AppState>,
    host: String,
) -> DesktopResult<Vec<ServerInfo>> {
    let client = state.client();
    let prog_ids = client.list_servers(&host).await?;
    Ok(prog_ids
        .into_iter()
        .map(|p| ServerInfo { prog_id: p })
        .collect())
}

/// Bind a ProgID. Subsequent data-plane commands operate on this server
/// until `disconnect` is called or another ProgID is bound.
#[tauri::command]
pub async fn connect(state: State<'_, AppState>, prog_id: String) -> DesktopResult<()> {
    state.set_prog_id(prog_id).await;
    Ok(())
}

/// Drop the current ProgID binding.
#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> DesktopResult<()> {
    state.clear_prog_id().await;
    Ok(())
}