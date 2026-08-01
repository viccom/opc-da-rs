//! Shared application state held inside the Tauri runtime.
//!
//! `opc_da_client::OpcProvider` is the stable async trait; the concrete
//! `OpcDaClient` is the production binding. The client itself is
//! stateless across ProgIDs and resolves connections lazily on each
//! call (the worker thread keeps a per-ProgID cache). It is NOT
//! stateless across hosts, though: `ComConnector` bakes the target host
//! in at construction and moves it into the worker thread, so switching
//! hosts means rebuilding the whole client (see [`AppState::rebuild_client`]).
//!
//! We store:
//! - the `Arc<OpcDaClient>` behind a `Mutex` so it can be swapped on a
//!   host change; commands clone it cheaply via [`AppState::client`],
//! - `current_host` for rebuild dedup,
//! - the currently-bound `ProgID` so data-plane commands know which
//!   server to operate on,
//! - a set of active subscription cookies (the actual `rx` lives inside
//!   each runner task — see [`crate::ipc::subscription_runner`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::Mutex;

use opc_da_client::{
    AuthCredentials, ComConnector, FusionReader, OpcDaClient, OpcProvider, SubscriptionHandle,
};

use crate::error::{DesktopError, DesktopResult};

/// Total budget for tearing down the old client's subscriptions during a host
/// rebuild. Per-subscription COM calls are uncancellable, so this caps the
/// whole teardown loop (the rebuild itself returns right after swapping in the
/// new client). Anything unfinished in time self-heals when the blocking COM
/// call eventually returns.
const REBUILD_TEARDOWN_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

/// Shared mutable state injected into every `#[tauri::command]` via
/// `tauri::State<'_, AppState>`.
pub struct AppState {
    /// The OPC client (always present after construction), wrapped so it can
    /// be swapped on a host change. Cheap to `Arc::clone` into spawned tokio
    /// tasks via [`AppState::client`].
    client: Mutex<Arc<OpcDaClient>>,

    /// Host the current `client` is bound to (baked into its `ComConnector`).
    /// A host change requires a full client rebuild — see
    /// [`AppState::rebuild_client`].
    current_host: Mutex<String>,

    /// ProgID the UI has currently "connected" to. `None` until the user
    /// picks a server.
    connected_prog_id: Mutex<Option<String>>,

    /// Cookies of active subscriptions (the actual `rx` lives inside each
    /// runner task). Used by `unsubscribe_tags` for validation.
    active_cookies: Mutex<HashSet<u32>>,

    /// DCOM credentials for the current client (`None` = current logged-in
    /// user). Snapshotted so `subscribe_fusion_tags` (which builds its own
    /// client via `FusionReader::start`) reuses the same credentials.
    credentials: Mutex<Option<AuthCredentials>>,

    /// Active FusionReader handles (id → reader). Drop = graceful unsubscribe
    /// via the reader's shutdown path. Cleared on host rebuild, like
    /// `active_cookies`.
    fusion_readers: Mutex<HashMap<u32, FusionReader>>,

    /// Next fusion subscription id (FusionReader has no server cookie; we
    /// synthesize a handle id to report to the frontend).
    next_fusion_id: AtomicU32,
}

impl AppState {
    /// Build a new `AppState` with a freshly-initialized `OpcDaClient`.
    /// On Windows this starts the COM worker thread; on other platforms
    /// `OpcDaClient::new` returns an `Internal` error.
    pub fn new() -> DesktopResult<Self> {
        let client = OpcDaClient::new(ComConnector::default()).map_err(DesktopError::from)?;
        Ok(Self {
            client: Mutex::new(Arc::new(client)),
            current_host: Mutex::new("localhost".to_string()),
            connected_prog_id: Mutex::new(None),
            active_cookies: Mutex::new(HashSet::new()),
            credentials: Mutex::new(None),
            fusion_readers: Mutex::new(HashMap::new()),
            next_fusion_id: AtomicU32::new(1),
        })
    }

    /// Clone the current client. Cheap (`Arc::clone`); the lock is held only
    /// for the clone. Commands must re-fetch this on every call so they pick
    /// up a rebuilt client after a host switch.
    pub async fn client(&self) -> Arc<OpcDaClient> {
        self.client.lock().await.clone()
    }

    /// Switch the bound host, rebuilding the client from scratch.
    ///
    /// `ComConnector` bakes the host in at construction and moves it into a
    /// dedicated worker thread, so a host change needs a brand-new
    /// `OpcDaClient` (new connector + worker). Flow:
    /// 1. no-op if `host` already matches `current_host`;
    /// 2. build the new client first (fail-fast: on error nothing is torn
    ///    down, so the old client and its subscriptions stay intact);
    /// 3. under the client lock: snapshot the old client + active cookies,
    ///    swap in the new client, clear host-bound state (cookies, ProgID);
    /// 4. **release the lock**, then tear down the old client's subscriptions
    ///    outside the lock (capped by [`REBUILD_TEARDOWN_BUDGET`]) so the data
    ///    plane can proceed on the new host without waiting on the old one.
    ///
    /// [`AppState::subscribe_atomic`] / [`AppState::unsubscribe_atomic`] each
    /// hold the client lock across their whole subscribe+register / forget+
    /// unsubscribe pair, so the swap window in step 3 can't race with a
    /// subscription registering on (or being forgotten from) the stale client.
    ///
    /// # Errors
    /// Returns [`DesktopError`] only if the new worker thread / COM init fails.
    //
    // `client_guard` is held across the swap + state clears deliberately so
    // they stay atomic w.r.t. `subscribe_atomic`/`unsubscribe_atomic` (same
    // lock). It is dropped before the potentially-long teardown loop. clippy's
    // `significant_drop_tightening` can't see that intent, so allow it here.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn rebuild_client(
        &self,
        host: &str,
        creds: Option<AuthCredentials>,
    ) -> DesktopResult<()> {
        let mut client_guard = self.client.lock().await;

        // Dedup on host AND credentials. `ComConnector` bakes both host and
        // credentials into the worker thread, so a credential change needs a
        // rebuild just like a host change. Compare credentials field-by-field
        // (`AuthCredentials` has no `PartialEq` — we don't want to expand its
        // public derive surface just for this dedup).
        {
            let current = self.current_host.lock().await;
            let current_creds = self.credentials.lock().await;
            let same_creds = match (&*current_creds, &creds) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.user == b.user && a.password == b.password && a.domain == b.domain
                }
                _ => false,
            };
            if current.as_str() == host && same_creds {
                return Ok(());
            }
        }

        // Build the new client first. If this fails we return early: the old
        // client stays in place and nothing is torn down.
        let new_client = Arc::new(
            match &creds {
                Some(c) => OpcDaClient::with_credentials(host, c.clone()),
                None => OpcDaClient::new(ComConnector::new(host)),
            }
            .map_err(DesktopError::from)?,
        );

        // Snapshot the old client + cookies, swap in the new client, and clear
        // host-bound state — all under the lock, but only these quick steps
        // (no COM calls here). subscribe_atomic/unsubscribe_atomic hold the
        // same lock, so they can't observe or mutate state mid-swap.
        let old_client = Arc::clone(&client_guard);
        let cookies: Vec<u32> = self.active_cookies.lock().await.iter().copied().collect();
        *client_guard = new_client;
        self.active_cookies.lock().await.clear();
        // FusionReaders bake host+creds into their own clients — drop them all
        // (each Drop gracefully unsubscribes its server-side group).
        self.fusion_readers.lock().await.clear();
        *self.credentials.lock().await = creds;
        *self.connected_prog_id.lock().await = None;
        *self.current_host.lock().await = host.to_string();
        drop(client_guard);

        // Tear down the old client's subscriptions WITHOUT the client lock so
        // the data plane can proceed on the new host. The old worker is
        // request-sequential and COM calls are uncancellable, so cap the whole
        // loop with a total budget; anything unfinished self-heals when the
        // blocking COM call returns.
        let _ = tokio::time::timeout(REBUILD_TEARDOWN_BUDGET, async {
            for cookie in cookies {
                let _ = old_client.unsubscribe(cookie).await;
            }
        })
        .await;
        Ok(())
    }

    /// Subscribe + register the cookie under the client lock, so the pair is
    /// atomic w.r.t. [`Self::rebuild_client`] (which clears `active_cookies`
    /// under the same lock). Returns the client `Arc` (for the runner to hold)
    /// and the subscription handle.
    ///
    /// # Errors
    /// Returns [`DesktopError`] if the server-side subscribe fails.
    //
    // The lock is held across the (potentially slow) COM `subscribe` so a
    // concurrent `rebuild_client` can't swap the client + clear cookies
    // between subscribe and register. clippy's `significant_drop_tightening`
    // can't see that intent, so allow it.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn subscribe_atomic(
        &self,
        prog_id: &str,
        tag_ids: Vec<String>,
        update_rate_ms: u32,
    ) -> DesktopResult<(Arc<OpcDaClient>, SubscriptionHandle)> {
        let client_guard = self.client.lock().await;
        let client = Arc::clone(&client_guard);
        let handle = client.subscribe(prog_id, tag_ids, update_rate_ms).await?;
        self.active_cookies.lock().await.insert(handle.cookie);
        Ok((client, handle))
    }

    /// Forget the cookie + unsubscribe under the client lock, atomic w.r.t.
    /// [`Self::rebuild_client`].
    ///
    /// # Errors
    /// Returns [`DesktopError::NotFound`] if `cookie` isn't active.
    //
    // Same rationale as `subscribe_atomic`: the lock spans the COM `unsubscribe`
    // so `rebuild_client` can't clear `active_cookies` between forget and
    // unsubscribe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn unsubscribe_atomic(&self, cookie: u32) -> DesktopResult<()> {
        let client_guard = self.client.lock().await;
        if !self.active_cookies.lock().await.remove(&cookie) {
            return Err(DesktopError::NotFound(format!("subscription {cookie}")));
        }
        let client = Arc::clone(&client_guard);
        if let Err(e) = client.unsubscribe(cookie).await {
            // Log but don't fail — the cookie is already forgotten, and the
            // runner exits on its own once the COM group is reaped server-side.
            tracing::warn!(cookie, error = %e, "client.unsubscribe failed during UI stop");
        }
        Ok(())
    }

    /// Snapshot the bound host (for `subscribe_fusion_tags`, which builds its
    /// own client via `FusionReader::start` and bypasses `AppState::client`).
    pub async fn host_snapshot(&self) -> String {
        self.current_host.lock().await.clone()
    }

    /// Snapshot the bound credentials (`None` = current logged-in user).
    pub async fn credentials_snapshot(&self) -> Option<AuthCredentials> {
        self.credentials.lock().await.clone()
    }

    /// Register a FusionReader handle, returning its synthetic id (reported
    /// to the frontend as the subscription "cookie").
    pub async fn register_fusion_reader(&self, reader: FusionReader) -> u32 {
        let id = self.next_fusion_id.fetch_add(1, Ordering::Relaxed);
        self.fusion_readers.lock().await.insert(id, reader);
        id
    }

    /// Drop a FusionReader (its `Drop` gracefully unsubscribes the server group).
    pub async fn remove_fusion_reader(&self, id: u32) {
        self.fusion_readers.lock().await.remove(&id);
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

    /// Stop ALL data streams: unsubscribe every active cookie + drop every
    /// FusionReader. The client itself stays in place (lazy OPC connection
    /// stays cached for the next connect) — this only halts subscriptions.
    /// Called by `disconnect` so the data plane actually stops.
    pub async fn stop_all_subscriptions(&self) {
        // Snapshot + clear cookies/fusion readers under short locks, then
        // unsubscribe outside any lock (COM calls are uncancellable).
        let cookies: Vec<u32> = self.active_cookies.lock().await.drain().collect();
        self.fusion_readers.lock().await.clear();
        let client = self.client.lock().await.clone();
        for cookie in cookies {
            let _ = client.unsubscribe(cookie).await;
        }
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
