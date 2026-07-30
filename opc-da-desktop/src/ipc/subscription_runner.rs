//! Subscription runner: drains `SubscriptionHandle::rx` and pushes
//! every value into a Tauri `Channel<TagUpdate>`.
//!
//! Lifecycle:
//! 1. Spawned by the `subscribe_tags` IPC command. Takes ownership of:
//!    - the `Arc<OpcDaClient>` (so it can call `unsubscribe` on teardown),
//!    - the `tokio::sync::mpsc::Receiver<TagValue>` (the only one in
//!      existence for this subscription),
//!    - the `Channel<TagUpdate>` that the WebView is listening on.
//! 2. Drains `rx` until either the stream ends naturally (server
//!    stopped callback) or the WebView side closes the channel
//!    (frontend disconnect / navigation).
//! 3. On exit, calls `client.unsubscribe(cookie)` to release the
//!    server-side group.

use std::sync::Arc;
use std::time::Duration;

use tauri::ipc::Channel;

use opc_da_client::{OpcDaClient, OpcProvider, TagValue};

use crate::ipc::TagUpdate;

/// Run one subscription. Returns when the stream ends, the channel is
/// closed, or a non-recoverable error occurs.
pub async fn run_subscription(
    client: Arc<OpcDaClient>,
    cookie: u32,
    mut rx: tauri::async_runtime::Receiver<TagValue>,
    channel: Channel<TagUpdate>,
) {
    tracing::info!(cookie, "subscription runner started");
    loop {
        let next = tokio::select! {
            biased;
            next = rx.recv() => next,
            _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
        };
        match next {
            Some(tag_value) => {
                if channel.send(TagUpdate::from(tag_value)).is_err() {
                    tracing::warn!(cookie, "channel send failed; WebView likely disconnected");
                    break;
                }
            },
            None => {
                tracing::info!(cookie, "subscription stream ended");
                break;
            },
        }
    }

    if let Err(e) = client.unsubscribe(cookie).await {
        tracing::warn!(cookie, error = %e, "unsubscribe failed during runner teardown");
    }
    tracing::info!(cookie, "subscription runner exited");
}