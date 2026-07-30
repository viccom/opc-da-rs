//! Subscription IPC handlers.
//!
//! `opc-da-client` 0.3.0's `OpcProvider::subscribe(server, tag_ids,
//! update_rate)` returns a [`SubscriptionHandle`] whose `rx` is a
//! non-cloneable `tokio::sync::mpsc::Receiver` (re-exported by
//! `tauri::async_runtime::Receiver` under Tauri 2's default runtime).
//! We immediately move the receiver into a spawned runner task along
//! with the `Arc<OpcDaClient>` (so the runner can call `unsubscribe`
//! on teardown) and a Tauri `Channel<TagUpdate>` (so the WebView can
//! receive the stream).
//!
//! ## Teardown contract (P0 audit fix)
//!
//! `unsubscribe_tags` MUST call `client.unsubscribe(cookie)` itself
//! before returning. The runner is passive — it only drains `rx` — so
//! without that explicit call:
//!   - `rx` never reaches `None`,
//!   - the runner never exits,
//!   - the OPC server's COM group + DataCallback sink are leaked,
//!   - cookies accumulate on the server until a restart.
//!
//! The runner's own `client.unsubscribe(cookie)` in
//! `subscription_runner.rs` only runs as a defensive cleanup on the
//! natural-exit path (channel closed, stream ended).

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use opc_da_client::OpcProvider;

use crate::error::{DesktopError, DesktopResult};
use crate::ipc::subscription_runner::run_subscription;
use crate::ipc::TagUpdate;
use crate::state::AppState;

/// Result returned by `subscribe_tags`: the cookie used to identify
/// the subscription in subsequent calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionCreated {
    /// Server-assigned cookie identifying this subscription.
    pub cookie: u32,
    /// Number of tags subscribed.
    pub tag_count: usize,
}

/// Subscribe to a set of tags. Tag updates flow over `channel` as a
/// stream of [`TagUpdate`] messages.
///
/// Refuses to start a second subscription while one is already active —
/// the UI is single-subscription by design (one group, one table).
#[tauri::command]
pub async fn subscribe_tags(
    state: State<'_, AppState>,
    tag_ids: Vec<String>,
    update_rate_ms: u32,
    channel: Channel<TagUpdate>,
) -> DesktopResult<SubscriptionCreated> {
    // P1-12: refuse overlapping subscriptions to avoid the dual-runner
    // scenario where one UI subscription "wins" while a hidden runner
    // continues consuming server resources.
    {
        let active = state.inner().active_cookies_snapshot().await;
        if !active.is_empty() {
            return Err(DesktopError::Other(format!(
                "a subscription is already active (cookies: {active:?}); stop it first"
            )));
        }
    }

    let client = state.client();
    let prog_id = state.prog_id().await?;
    let handle = client
        .subscribe(&prog_id, tag_ids.clone(), update_rate_ms)
        .await?;

    let cookie = handle.cookie;
    let tag_count = tag_ids.len();
    let rx = handle.rx;

    state.register_cookie(cookie).await;

    // Spawn the runner that pumps rx → channel. Detached so the IPC
    // command returns immediately. The runner drains `rx`; once the
    // channel closes (frontend disconnect) or `rx` reaches `None`,
    // the runner calls `client.unsubscribe(cookie)` as a defensive
    // cleanup. Normal teardown goes through `unsubscribe_tags` below.
    tokio::spawn(run_subscription(client, cookie, rx, channel));

    Ok(SubscriptionCreated { cookie, tag_count })
}

/// Cancel a subscription.
///
/// P0 fix: actually invoke `client.unsubscribe(cookie)` so the server
/// releases the COM group and the runner's `rx` reaches `None` — the
/// runner then exits on its own.
#[tauri::command]
pub async fn unsubscribe_tags(
    state: State<'_, AppState>,
    cookie: u32,
) -> DesktopResult<()> {
    if !state.forget_cookie(cookie).await {
        return Err(DesktopError::NotFound(format!("subscription {cookie}")));
    }

    // Drop the server-side subscription. This causes the worker's
    // `tx` to be dropped, which propagates `None` to `rx`, which the
    // runner observes on its next iteration.
    let client = state.client();
    if let Err(e) = client.unsubscribe(cookie).await {
        // Log but don't fail — the cookie is already forgotten, and the
        // runner will exit on its own once the COM group is reaped
        // server-side. Re-raising here would leave the UI in a state
        // where "Stop" appears to have failed even though it succeeded
        // from the user's perspective.
        tracing::warn!(cookie, error = %e, "client.unsubscribe failed during UI stop");
    }
    Ok(())
}