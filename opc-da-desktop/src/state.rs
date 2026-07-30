//! Shared application state held inside the Tauri runtime.
//!
//! `opc_da_client::OpcProvider` is the stable async trait; the concrete
//! `OpcDaClient` is the production binding. The client itself is
//! stateless across ProgIDs and resolves connections lazily on each
//! call (the worker thread keeps a per-ProgID cache).
//!
//! We store:
//! - the `Arc<OpcDaClient>` so commands can clone it cheaply,
//! - the currently-bound `ProgID` so data-plane commands know which
//!   server to operate on,
//! - a set of active subscription cookies (the actual `rx` lives inside
//!   each runner task — see [`crate::ipc::subscription_runner`]).

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

use opc_da_client::{ComConnector, OpcDaClient};

use crate::error::{DesktopError, DesktopResult};

/// Shared mutable state injected into every `#[tauri::command]` via
/// `tauri::State<'_, AppState>`.
pub struct AppState {
    /// The OPC client (always present after construction). Cheap to
    /// `Arc::clone` into spawned tokio tasks.
    client: Arc<OpcDaClient>,

    /// ProgID the UI has currently "connected" to. `None` until the user
    /// picks a server.
    connected_prog_id: Mutex<Option<String>>,

    /// Cookies of active subscriptions (the actual `rx` lives inside each
    /// runner task). Used by `unsubscribe_tags` for validation.
    active_cookies: Mutex<HashSet<u32>>,
}

impl AppState {
    /// Build a new `AppState` with a freshly-initialized `OpcDaClient`.
    /// On Windows this starts the COM worker thread; on other platforms
    /// `OpcDaClient::new` returns an `Internal` error.
    pub fn new() -> DesktopResult<Self> {
        let client = OpcDaClient::new(ComConnector::default()).map_err(DesktopError::from)?;
        Ok(Self {
            client: Arc::new(client),
            connected_prog_id: Mutex::new(None),
            active_cookies: Mutex::new(HashSet::new()),
        })
    }

    /// Borrow the connected client (always non-None).
    pub fn client(&self) -> Arc<OpcDaClient> {
        Arc::clone(&self.client)
    }

    /// Currently-bound ProgID, or [`DesktopError::NotConnected`] if the
    /// UI hasn't picked one yet.
    pub async fn prog_id(&self) -> DesktopResult<String> {
        self.connected_prog_id
            .lock()
            .await
            .clone()
            .ok_or(DesktopError::NotConnected)
    }

    /// Bind a ProgID.
    pub async fn set_prog_id(&self, prog_id: String) {
        *self.connected_prog_id.lock().await = Some(prog_id);
    }

    /// Drop the current ProgID binding.
    pub async fn clear_prog_id(&self) {
        *self.connected_prog_id.lock().await = None;
    }

    /// Record a subscription cookie as active.
    pub async fn register_cookie(&self, cookie: u32) {
        self.active_cookies.lock().await.insert(cookie);
    }

    /// Forget a subscription cookie. Returns `false` if it wasn't active.
    pub async fn forget_cookie(&self, cookie: u32) -> bool {
        self.active_cookies.lock().await.remove(&cookie)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new().expect("failed to start OPC COM worker thread")
    }
}
