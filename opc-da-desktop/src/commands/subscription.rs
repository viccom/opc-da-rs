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

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;

use opc_da_client::{FusionReader, FusionReaderOptions};

use crate::error::DesktopResult;
use crate::ipc::fusion_runner::run_fusion_subscription;
use crate::ipc::subscription_runner::run_subscription;
use crate::ipc::{FusionEventDto, TagUpdate};
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
/// Multiple concurrent subscriptions are supported — each gets its own
/// OPC group + cookie + `Channel`. The frontend associates each with a
/// client group id (see the desktop UI's group sidebar).
#[tauri::command]
pub async fn subscribe_tags(
    state: State<'_, AppState>,
    tag_ids: Vec<String>,
    update_rate_ms: u32,
    channel: Channel<TagUpdate>,
) -> DesktopResult<SubscriptionCreated> {
    let prog_id = state.prog_id().await?;
    // subscribe + register the cookie atomically under the client lock, so a
    // concurrent host rebuild can't swap the client / clear cookies between
    // subscribe and register. Returns the client Arc for the runner to hold.
    let (client, handle) = state
        .subscribe_atomic(&prog_id, tag_ids.clone(), update_rate_ms)
        .await?;

    let cookie = handle.cookie;
    let tag_count = tag_ids.len();
    let rx = handle.rx;

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
pub async fn unsubscribe_tags(state: State<'_, AppState>, cookie: u32) -> DesktopResult<()> {
    // forget the cookie + drop the server-side subscription atomically under
    // the client lock (`AppState::unsubscribe_atomic`), so a concurrent host
    // rebuild can't clear `active_cookies` between the two. Dropping the
    // server-side subscription closes the worker's `tx` → the runner's `rx`
    // reaches `None` → the runner exits on its own.
    state.unsubscribe_atomic(cookie).await?;
    Ok(())
}

/// Subscribe via `FusionReader`（订阅优先，订阅不通自动同步兜底）。
///
/// 与 [`subscribe_tags`] 不同：用独立 client（`FusionReader::start` 自建，绕过
/// `AppState::client`），事件流为 [`FusionEventDto`]（`Data` / `Subscribed` /
/// `Fallback`）。返回的 `cookie` 实为 fusion 句柄 id，供 [`unsubscribe_fusion_tags`]
/// drop reader（其 `Drop` 优雅退订 server 端订阅）。
#[tauri::command]
pub async fn subscribe_fusion_tags(
    state: State<'_, AppState>,
    tag_ids: Vec<String>,
    update_rate_ms: u32,
    channel: Channel<FusionEventDto>,
) -> DesktopResult<SubscriptionCreated> {
    let prog_id = state.prog_id().await?;
    let host = state.host_snapshot().await;
    let creds = state.credentials_snapshot().await;
    let opts = FusionReaderOptions {
        update_rate: update_rate_ms,
        fallback_timeout: Duration::from_secs(10),
        buffer: 256,
    };
    let (reader, rx) = FusionReader::start(&host, creds, &prog_id, tag_ids.clone(), &opts)?;
    let sub_id = state.register_fusion_reader(reader).await;
    tokio::spawn(run_fusion_subscription(rx, channel));
    Ok(SubscriptionCreated {
        cookie: sub_id,
        tag_count: tag_ids.len(),
    })
}

/// Drop a FusionReader（其 `Drop` 优雅退订 server 端订阅）。
#[tauri::command]
pub async fn unsubscribe_fusion_tags(state: State<'_, AppState>, sub_id: u32) -> DesktopResult<()> {
    state.remove_fusion_reader(sub_id).await;
    Ok(())
}
