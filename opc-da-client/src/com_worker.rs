use crate::backend::connector::{ConnectedGroup, ConnectedServer, ServerConnector};
use crate::bindings::da::{
    OPC_BRANCH, OPC_BROWSE_DOWN, OPC_BROWSE_UP, OPC_DS_DEVICE, OPC_FLAT, OPC_LEAF, OPC_NS_FLAT,
    tagOPCITEMDEF,
};
use crate::helpers::{
    filetime_to_string, format_hresult, opc_value_to_variant, quality_to_string, variant_to_string,
};
use crate::opc_da::com_utils::clear_item_states;
use crate::opc_da::errors::{OpcError, OpcResult};
use crate::opc_da::typedefs::ServerStatus;
use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
use crate::provider::{OpcValue, ShutdownHandle, SubscriptionHandle, TagValue, WriteResult};
use crate::subscription::{DataCallbackSink, ShutdownSink};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use windows::Win32::System::Variant::VariantClear;
use windows::core::Interface as _;

/// Represents a asynchronous request dispatched to the COM worker thread.
pub enum ComRequest {
    /// Request to enumerate available OPC DA servers on a host.
    ListServers {
        /// Hostname or IP address to target.
        host: String,
        /// One-shot channel to send back the server enumeration result.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request to read current values, quality, and timestamps for tag IDs.
    ReadTagValues {
        /// OPC server ProgID.
        server: String,
        /// List of fully qualified tag identifiers to read.
        tag_ids: Vec<String>,
        /// One-shot channel to send back the tag values result.
        reply: oneshot::Sender<OpcResult<Vec<TagValue>>>,
    },
    /// Request to write a typed value to a single tag.
    WriteTagValue {
        /// OPC server ProgID.
        server: String,
        /// Tag identifier to write.
        tag_id: String,
        /// Typed value to write.
        value: OpcValue,
        /// One-shot channel to send back the write operation result.
        reply: oneshot::Sender<OpcResult<WriteResult>>,
    },
    /// Request to recursively browse available tags on a server.
    BrowseTags {
        /// OPC server ProgID.
        server: String,
        /// Maximum number of tags to discover before stopping.
        max_tags: usize,
        /// Atomic counter tracking total tags discovered.
        progress: Arc<AtomicUsize>,
        /// Shared mutex-protected vector storing discovered tag names incrementally.
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        /// Filter: requested canonical data type (0 = any).
        data_type: u16,
        /// Filter: required access rights (0 = any).
        access_rights: u32,
        /// One-shot channel to send back the complete tag discovery list.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request to query the current server status (`IOPCServer::GetStatus`).
    GetServerStatus {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the server status.
        reply: oneshot::Sender<OpcResult<ServerStatus>>,
    },
    /// Request to write values to multiple tags in one operation.
    WriteTagValues {
        /// OPC server ProgID.
        server: String,
        /// `(tag_id, value)` pairs to write.
        items: Vec<(String, OpcValue)>,
        /// One-shot channel to send back the per-tag write results.
        reply: oneshot::Sender<OpcResult<Vec<WriteResult>>>,
    },
    /// Request to query item properties (`IOPCItemProperties`).
    GetItemProperties {
        /// OPC server ProgID.
        server: String,
        /// Fully qualified item ID.
        tag_id: String,
        /// One-shot channel to send back the item properties.
        reply: oneshot::Sender<OpcResult<Vec<crate::provider::ItemProperty>>>,
    },
    /// Request to read values with a maximum-age constraint (`IOPCSyncIO2::ReadMaxAge`).
    ReadMaxAge {
        /// OPC server ProgID.
        server: String,
        /// Tag IDs to read.
        tag_ids: Vec<String>,
        /// Maximum acceptable value age in milliseconds.
        max_age_ms: u32,
        /// One-shot channel to send back the tag values.
        reply: oneshot::Sender<OpcResult<Vec<TagValue>>>,
    },
    /// Request to write a value with quality/timestamp (`IOPCSyncIO2::WriteVQT`).
    WriteTagValueVqt {
        /// OPC server ProgID.
        server: String,
        /// Tag identifier to write.
        tag_id: String,
        /// Typed value to write.
        value: OpcValue,
        /// Optional OPC quality bits. `None` = leave unset.
        quality: Option<u16>,
        /// Optional timestamp. `None` = leave unset.
        timestamp: Option<std::time::SystemTime>,
        /// One-shot channel to send back the write result.
        reply: oneshot::Sender<OpcResult<WriteResult>>,
    },
    /// Request to get a server-localized error string (`IOPCCommon::GetErrorString`).
    GetErrorString {
        /// OPC server ProgID.
        server: String,
        /// Raw HRESULT as a signed 32-bit integer.
        hresult: i32,
        /// One-shot channel to send back the error string.
        reply: oneshot::Sender<OpcResult<String>>,
    },
    /// Request to drop a cached server connection (next op reconnects).
    Disconnect {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to acknowledge.
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to force a fresh server connection (replaces cached).
    Reconnect {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the outcome.
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to subscribe to data changes (`IOPCDataCallback`).
    Subscribe {
        /// OPC server ProgID.
        server: String,
        /// Tag IDs to monitor.
        tag_ids: Vec<String>,
        /// Requested group update rate in milliseconds.
        update_rate: u32,
        /// One-shot channel to send back the subscription handle.
        reply: oneshot::Sender<OpcResult<SubscriptionHandle>>,
    },
    /// Request to tear down a subscription by cookie.
    Unsubscribe {
        /// Connection cookie returned by `Subscribe`.
        cookie: u32,
        /// One-shot channel to acknowledge.
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to rebuild a subscription's callback sink after a detected RPC drop (P0-1).
    ///
    /// Re-uses the client `rx` (kept open by the entry's original `tx`) while swapping the
    /// underlying COM sink. Emitted by the health monitor when `last_update` goes stale.
    RebuildSubscription {
        /// Connection cookie returned by `Subscribe` (stable client handle; the live COM
        /// advise cookie is tracked separately inside the `SubscriptionEntry`).
        cookie: u32,
        /// One-shot channel to acknowledge (carries rebuild failure for step E).
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to subscribe to server shutdown notifications (`IOPCShutdown`).
    SubscribeShutdown {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the shutdown handle.
        reply: oneshot::Sender<OpcResult<ShutdownHandle>>,
    },
    /// Request to tear down a shutdown subscription by cookie.
    UnsubscribeShutdown {
        /// Connection cookie returned by `SubscribeShutdown`.
        cookie: u32,
        /// One-shot channel to acknowledge.
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to change an active subscription's update rate (`IOPCGroupStateMgt::SetState`).
    SetSubscriptionRate {
        /// Connection cookie returned by `Subscribe`.
        cookie: u32,
        /// Requested update rate in milliseconds.
        update_rate: u32,
        /// One-shot channel to send back the server-revised update rate.
        reply: oneshot::Sender<OpcResult<u32>>,
    },
    /// Request to set keep-alive on an active subscription (`IOPCGroupStateMgt2::SetKeepAlive`).
    SetKeepAlive {
        /// Connection cookie returned by `Subscribe`.
        cookie: u32,
        /// Requested keep-alive interval in milliseconds (0 = disable).
        keep_alive_ms: u32,
        /// One-shot channel to send back the server-revised keep-alive interval.
        reply: oneshot::Sender<OpcResult<u32>>,
    },
    /// Request to set the server locale (`IOPCCommon::SetLocaleID`).
    SetLocaleId {
        /// OPC server ProgID.
        server: String,
        /// Windows LCID.
        locale_id: u32,
        /// One-shot channel to acknowledge.
        reply: oneshot::Sender<OpcResult<()>>,
    },
    /// Request to set the client name (`IOPCCommon::SetClientName`).
    SetClientName {
        /// OPC server ProgID.
        server: String,
        /// Client application name.
        name: String,
        /// One-shot channel to acknowledge.
        reply: oneshot::Sender<OpcResult<()>>,
    },
}

/// Dedicated background worker thread manager handling COM MTA apartment thread affinity.
///
/// Dispatches requests received over an `mpsc` channel to Windows COM interfaces while maintaining
/// a persistent connection pool and transparently evicting stale connection handles on RPC errors.
/// Worker-side tracked state for an active subscription.
///
/// Kept alive so the group is not removed and the sink is not released until `Unsubscribe`.
struct SubscriptionEntry<C: ServerConnector + 'static> {
    /// ProgID owning the group (used to look the server up in cache for `remove_group`).
    server_name: String,
    /// Server-assigned group handle (for `remove_group` on teardown).
    server_handle: GroupHandle,
    /// The persistent group owning the advised callback.
    group: <<C as ServerConnector>::Server as ConnectedServer>::Group,
    /// Held reference to the COM sink; dropping this field (on `Unsubscribe`) releases the
    /// callback object. Intentionally never read — its lifetime *is* the point.
    #[allow(dead_code)]
    sink: windows::core::IUnknown,
    /// Shared liveness timestamp (P0-1) for dead-callback detection; stamped by each
    /// `OnDataChange`, read by the health monitor.
    #[allow(dead_code)] // read by the health monitor, added in P0-1 step D
    last_update: Arc<AtomicU64>,
    /// Original mpsc sender kept alive so the client `rx` stays open across a sink rebuild
    /// (P0-1 step B): each `DataCallbackSink` only holds a clone, so releasing the old sink
    /// during rebuild does not close the channel.
    tx: mpsc::Sender<TagValue>,
    /// Current COM `IConnectionPoint::Advise` cookie (P0-1 step C). Decoupled from the stable
    /// client cookie (the map key) because each rebuild re-advises and gets a fresh one.
    com_cookie: u32,
    /// Subscribed tag IDs (P0-1 step C), kept so a rebuilt sink can rebuild its client-handle
    /// → tag-id map without re-adding items to the group.
    tag_ids: Vec<String>,
    /// Original error-channel sender (P0-1 step E): the worker pushes a rebuild-failure
    /// `OpcError` here so the client's `SubscriptionHandle.errors` can surface it. Best-effort.
    error_tx: mpsc::Sender<OpcError>,
    /// Original requested update rate (P0-1 增强): rebuild's reconnect path re-creates the
    /// group with this same cadence after a dead-server reconnect.
    update_rate: u32,
}

/// Worker-side tracked state for a shutdown subscription (server-level; no group).
struct ShutdownEntry {
    server_name: String,
    #[allow(dead_code)]
    sink: windows::core::IUnknown,
}

/// Tuning parameters for the subscription health monitor thread (P0-1 step D).
///
/// Intentionally no `Default` impl: a zero `period` would busy-loop the monitor.
#[derive(Clone, Copy, Debug)]
pub struct HealthMonitorConfig {
    /// Sleep between scans of the subscription registry.
    pub period: Duration,
    /// Lower bound on the staleness threshold (see the timeout formula in
    /// `spawn_health_monitor`). Guards against false rebuilds for quiet DA 2.0 tags
    /// that legitimately go long without data changes.
    pub min_timeout: Duration,
}

impl HealthMonitorConfig {
    /// Production defaults: 1s scan cadence, 30s conservative staleness floor.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            period: Duration::from_secs(1),
            min_timeout: Duration::from_secs(30),
        }
    }
}

/// Per-subscription state the health monitor observes (P0-1 step D).
///
/// `last_update` is the **same `Arc`** as `SubscriptionEntry::last_update` (created
/// together in `handle_subscribe`), so the rebuild handler's `store` automatically
/// refreshes what the monitor reads.
struct MonitorEntry {
    last_update: Arc<AtomicU64>,
    update_rate_ms: u32,
    /// Pending rebuild reply (P0-1 step E): Some while a RebuildSubscription is in flight;
    /// the monitor `try_recv`s it next cycle to learn success/failure for backoff.
    pending: Option<oneshot::Receiver<OpcResult<()>>>,
    /// Consecutive rebuild failures (P0-1 step E); drives exponential backoff.
    consecutive_failures: u32,
    /// Epoch-ms before which rebuild should NOT be retried (P0-1 step E backoff window).
    next_retry_after_ms: u64,
}

pub struct ComWorker<C: ServerConnector + 'static> {
    /// Channel sender for dispatching requests to the worker loop.
    ///
    /// `pub(crate)` on purpose: `Drop` joins the worker thread, which only
    /// terminates once every sender is dropped. An externally cloned sender
    /// would keep the channel open and deadlock `join` — all requests must go
    /// through [`ComWorker::send_request`].
    pub(crate) sender: Option<mpsc::Sender<ComRequest>>,
    /// Thread join handle, joined in `Drop` for deterministic teardown.
    pub(crate) handle: Option<std::thread::JoinHandle<()>>,
    /// Last panic payload captured from the worker thread (if any). See
    /// [`ComWorker::captured_panic`].
    last_panic: Arc<std::sync::Mutex<Option<String>>>,
    /// Shared registry of active subscriptions the health monitor scans (P0-1 step D):
    /// cookie → (liveness timestamp Arc, update_rate). Worker inserts/removes; monitor reads.
    ///
    /// Held on `ComWorker` only for test observability (D-2/D-5 read it through the worker);
    /// the worker and monitor threads keep their own clones, so production code never reads
    /// this field directly.
    #[allow(dead_code)]
    monitor_registry: Arc<Mutex<HashMap<u32, MonitorEntry>>>,
    /// Health monitor thread join handle; joined in `Drop` *before* closing the worker
    /// channel so the monitor's `tx` clone is released first (else `blocking_recv` never
    /// returns `None` and the worker join deadlocks).
    monitor_handle: Option<std::thread::JoinHandle<()>>,
    /// Shutdown flag for the health monitor thread.
    monitor_shutdown: Arc<AtomicBool>,
    _phantom: std::marker::PhantomData<C>,
}

/// Extract a human-readable message from a panic payload captured by
/// `catch_unwind` / `JoinHandle::join`.
fn stringify_panic_payload(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Spawn the subscription health monitor thread (P0-1 step D).
///
/// Periodically scans the shared registry; for any subscription whose `last_update` is
/// older than `max(update_rate*3, min_timeout)`, fires a `RebuildSubscription` request
/// (fire-and-forget) so the worker rebuilds the dead callback sink. Touches no COM
/// pointers — only shared atomics + the request channel.
fn spawn_health_monitor(
    tx: mpsc::Sender<ComRequest>,
    registry: Arc<Mutex<HashMap<u32, MonitorEntry>>>,
    shutdown: Arc<AtomicBool>,
    config: HealthMonitorConfig,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        tracing::debug!(
            period = ?config.period,
            min_timeout = ?config.min_timeout,
            "subscription health monitor thread spawned"
        );
        while !shutdown.load(Ordering::Relaxed) {
            segmented_sleep(&shutdown, config.period);
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let now = now_ms();
            let min_timeout_ms = u32::try_from(config.min_timeout.as_millis()).unwrap_or(u32::MAX);
            // Under the lock: drain pending rebuild replies, apply backoff, collect stale
            // cookies. Time math is cheap and non-blocking; try_send runs after the unlock.
            let triggers: Vec<u32> = {
                let Ok(mut guard) = registry.lock() else {
                    continue;
                };
                guard
                    .iter_mut()
                    .filter_map(|(&cookie, entry)| {
                        // (1) Drain a pending rebuild reply (try_recv is non-blocking).
                        if let Some(mut rx) = entry.pending.take() {
                            match rx.try_recv() {
                                Ok(Ok(())) => {
                                    entry.consecutive_failures = 0;
                                    entry.next_retry_after_ms = 0;
                                }
                                Ok(Err(_)) => {
                                    entry.consecutive_failures =
                                        entry.consecutive_failures.saturating_add(1);
                                    entry.next_retry_after_ms = now
                                        + monitor_backoff_ms(
                                            entry.consecutive_failures,
                                            config.period,
                                        );
                                }
                                Err(_) => {
                                    entry.pending = Some(rx); // Empty: rebuild still in flight
                                    return None;
                                }
                            }
                        }
                        // (2) Backoff window: don't retry right after a failure.
                        if now < entry.next_retry_after_ms {
                            return None;
                        }
                        // (3) Staleness check.
                        let threshold = entry.update_rate_ms.saturating_mul(3).max(min_timeout_ms);
                        let stale = now.saturating_sub(entry.last_update.load(Ordering::Relaxed))
                            > u64::from(threshold);
                        stale.then_some(cookie)
                    })
                    .collect()
            };
            // Outside the lock: fire rebuilds, store the reply receiver for next cycle.
            for cookie in triggers {
                let (reply, reply_rx) = oneshot::channel();
                match tx.try_send(ComRequest::RebuildSubscription { cookie, reply }) {
                    Ok(()) => {
                        if let Ok(mut guard) = registry.lock()
                            && let Some(entry) = guard.get_mut(&cookie)
                        {
                            entry.pending = Some(reply_rx);
                            // Anti-storm: reset so we don't refire every cycle while the
                            // rebuild is in flight (handle_rebuild_subscription also resets).
                            entry.last_update.store(now, Ordering::Relaxed);
                        }
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(cookie, "worker channel full; skipping rebuild this cycle");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::debug!("worker channel closed; health monitor exiting");
                        return;
                    }
                }
            }
        }
        tracing::debug!("subscription health monitor thread exiting");
    })
}

/// Sleep for `total`, checking `shutdown` every 10ms so the monitor responds to teardown
/// within ~10ms instead of waiting a full period.
/// Exponential backoff (ms) for repeated rebuild failures (P0-1 step E): `period * 2^n`,
/// capped at 5 minutes so a recovering server is eventually retried.
fn monitor_backoff_ms(consecutive_failures: u32, period: Duration) -> u64 {
    let period_ms = u64::try_from(period.as_millis()).unwrap_or(u64::MAX);
    let n = consecutive_failures.min(8); // cap exponent
    period_ms.saturating_mul(1u64 << n).min(5 * 60 * 1000)
}

fn segmented_sleep(shutdown: &AtomicBool, total: Duration) {
    let chunk = Duration::from_millis(10);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        let step = chunk.min(remaining);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Current time as milliseconds since `UNIX_EPOCH` (0 on clock read failure).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

#[allow(clippy::cast_possible_wrap)]
fn is_connection_error(err: &OpcError) -> bool {
    if let OpcError::Com { source } = err {
        let code = source.code().0;
        code == windows::core::HRESULT(0x8007_06BA_u32 as i32).0
            || code == windows::core::HRESULT(0x8007_06BF_u32 as i32).0
            || code == windows::core::HRESULT(0x8007_06BE_u32 as i32).0
            || code == windows::core::HRESULT(0x8008_0005_u32 as i32).0
    } else {
        false
    }
}

impl<C: ServerConnector + 'static> ComWorker<C> {
    pub fn start(connector: Arc<C>) -> Result<Self, OpcError> {
        Self::start_with_health(connector, HealthMonitorConfig::production())
    }

    /// Like [`start`](Self::start) but with an explicit health-monitor configuration.
    ///
    /// Production callers use [`start`](Self::start); tests pass short tuning so the
    /// monitor triggers within a reasonable wall-clock window.
    #[allow(clippy::too_many_lines)]
    pub fn start_with_health(
        connector: Arc<C>,
        health: HealthMonitorConfig,
    ) -> Result<Self, OpcError> {
        let (tx, mut rx) = mpsc::channel(32);
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        // Shared cell capturing the worker thread's last panic payload (P0-3/P0-4).
        // Cross-thread so `captured_panic()` can report worker health to callers.
        let last_panic: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let worker_last_panic = Arc::clone(&last_panic);
        // P0-1 step D: shared subscription registry the health monitor scans, plus its
        // shutdown flag. The worker inserts/removes (step D-2); the monitor reads (D-3).
        let monitor_registry = Arc::new(Mutex::new(HashMap::<u32, MonitorEntry>::new()));
        let worker_registry = Arc::clone(&monitor_registry);
        let monitor_shutdown = Arc::new(AtomicBool::new(false));

        let handle = std::thread::spawn(move || {
            tracing::debug!("COM worker thread spawned, initializing COM (MTA)");
            let _guard = match crate::ComGuard::new() {
                Ok(g) => {
                    tracing::info!("COM MTA initialized successfully on worker thread");
                    let _ = init_tx.send(Ok(()));
                    g
                }
                Err(e) => {
                    tracing::error!(error = ?e, "COM worker failed to initialize MTA");
                    let _ =
                        init_tx.send(Err(OpcError::Internal("COM init failed on worker".into())));
                    return;
                }
            };

            let mut cache: HashMap<String, C::Server> = HashMap::new();
            let mut subscriptions: HashMap<u32, SubscriptionEntry<C>> = HashMap::new();
            let mut shutdown_subscriptions: HashMap<u32, ShutdownEntry> = HashMap::new();

            while let Some(req) = rx.blocking_recv() {
                // P0-4: catch panics inside the request handler so the root-cause
                // message is preserved (written to `worker_last_panic` + logged)
                // instead of tearing down the worker thread and losing all context.
                // SAFETY: `AssertUnwindSafe` is sound because on panic we `break`
                // and abandon `cache`/`subscriptions`/COM pointers — we never reuse
                // the possibly-corrupted mutable state afterwards.
                let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match req {
                        ComRequest::ListServers { host, reply } => {
                            let span = tracing::info_span!("opc.list_servers", host = %host);
                            let _enter = span.enter();
                            let start = std::time::Instant::now();
                            let servers = connector.enumerate_servers(&host);
                            if let Ok(s) = &servers {
                                tracing::info!(
                                    count = s.len(),
                                    elapsed_ms = u64::try_from(start.elapsed().as_millis())
                                        .unwrap_or(u64::MAX),
                                    "list_servers completed"
                                );
                            } else if let Err(e) = &servers {
                                tracing::error!(
                                    error = ?e,
                                    elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                                    "list_servers failed"
                                );
                            }
                            let _ = reply.send(servers);
                        }
                        ComRequest::ReadTagValues {
                            server,
                            tag_ids,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| Self::handle_read(&server, &tag_ids, opc_server),
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::WriteTagValue {
                            server,
                            tag_id,
                            value,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| {
                                    Self::handle_write(&server, &tag_id, &value, opc_server)
                                },
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::BrowseTags {
                            server,
                            max_tags,
                            progress,
                            tags_sink,
                            data_type,
                            access_rights,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| {
                                    Self::handle_browse(
                                        &server,
                                        max_tags,
                                        &progress,
                                        &tags_sink,
                                        data_type,
                                        access_rights,
                                        opc_server,
                                    )
                                },
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::GetServerStatus { server, reply } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                <C::Server as ConnectedServer>::get_status,
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::WriteTagValues {
                            server,
                            items,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| Self::handle_write_values(&server, &items, opc_server),
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::GetItemProperties {
                            server,
                            tag_id,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| opc_server.get_item_properties(&tag_id),
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::ReadMaxAge {
                            server,
                            tag_ids,
                            max_age_ms,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| {
                                    Self::handle_read_max_age(
                                        &server, &tag_ids, max_age_ms, opc_server,
                                    )
                                },
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::WriteTagValueVqt {
                            server,
                            tag_id,
                            value,
                            quality,
                            timestamp,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| {
                                    Self::handle_write_vqt(
                                        &server, &tag_id, &value, quality, timestamp, opc_server,
                                    )
                                },
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::GetErrorString {
                            server,
                            hresult,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| opc_server.get_error_string(hresult),
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::Disconnect { server, reply } => {
                            tracing::debug!(server = %server, "Explicit disconnect: evicting cached connection");
                            cache.remove(&server);
                            let _ = reply.send(Ok(()));
                        }
                        ComRequest::Reconnect { server, reply } => {
                            tracing::debug!(server = %server, "Explicit reconnect: re-establishing connection");
                            cache.remove(&server);
                            let result = connector.connect(&server).map(|srv| {
                            cache.insert(server.clone(), srv);
                        }).map_err(|e| {
                            tracing::warn!(error = ?e, server = %server, "explicit reconnect failed");
                            e
                        });
                            let _ = reply.send(result);
                        }
                        ComRequest::Subscribe {
                            server,
                            tag_ids,
                            update_rate,
                            reply,
                        } => {
                            let result = Self::handle_subscribe_request(
                                &connector,
                                &mut cache,
                                &mut subscriptions,
                                &worker_registry,
                                &server,
                                &tag_ids,
                                update_rate,
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::Unsubscribe { cookie, reply } => {
                            let result = Self::handle_unsubscribe(
                                cookie,
                                &cache,
                                &mut subscriptions,
                                &worker_registry,
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::RebuildSubscription { cookie, reply } => {
                            let result = Self::handle_rebuild_subscription(
                                cookie,
                                &mut subscriptions,
                                &mut cache,
                                &connector,
                            );
                            // P0-1 step E: surface a rebuild failure on the subscription's
                            // error channel so consumers can see it (not silent). Unknown cookie
                            // (entry gone) yields no signal — nothing to report to.
                            if let Err(ref e) = result
                                && let Some(entry) = subscriptions.get(&cookie)
                            {
                                let _ = entry.error_tx.try_send(OpcError::Internal(format!(
                                    "subscription rebuild failed: {e}"
                                )));
                            }
                            let _ = reply.send(result);
                        }
                        ComRequest::SubscribeShutdown { server, reply } => {
                            let result = (|| {
                                let srv = match cache.entry(server.clone()) {
                                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                                    std::collections::hash_map::Entry::Vacant(e) => {
                                        e.insert(connector.connect(&server)?)
                                    }
                                };
                                Self::handle_subscribe_shutdown(
                                    &server,
                                    srv,
                                    &mut shutdown_subscriptions,
                                )
                            })();
                            let _ = reply.send(result);
                        }
                        ComRequest::UnsubscribeShutdown { cookie, reply } => {
                            let result = Self::handle_unsubscribe_shutdown(
                                cookie,
                                &cache,
                                &mut shutdown_subscriptions,
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::SetSubscriptionRate {
                            cookie,
                            update_rate,
                            reply,
                        } => {
                            let result = match subscriptions.get_mut(&cookie) {
                                Some(entry) => entry.group.set_update_rate(update_rate),
                                None => Err(OpcError::InvalidState(format!(
                                    "unknown subscription cookie {cookie}"
                                ))),
                            };
                            let _ = reply.send(result);
                        }
                        ComRequest::SetKeepAlive {
                            cookie,
                            keep_alive_ms,
                            reply,
                        } => {
                            let result = match subscriptions.get_mut(&cookie) {
                                Some(entry) => entry.group.set_keep_alive(keep_alive_ms),
                                None => Err(OpcError::InvalidState(format!(
                                    "unknown subscription cookie {cookie}"
                                ))),
                            };
                            let _ = reply.send(result);
                        }
                        ComRequest::SetLocaleId {
                            server,
                            locale_id,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| opc_server.set_locale_id(locale_id),
                            );
                            let _ = reply.send(result);
                        }
                        ComRequest::SetClientName {
                            server,
                            name,
                            reply,
                        } => {
                            let result = Self::dispatch_with_retry(
                                &mut cache,
                                &connector,
                                &server,
                                |opc_server| opc_server.set_client_name(&name),
                            );
                            let _ = reply.send(result);
                        }
                    }
                }));
                if let Err(payload) = panic_result {
                    let msg = stringify_panic_payload(&payload);
                    if let Ok(mut guard) = worker_last_panic.lock() {
                        *guard = Some(msg.clone());
                    }
                    tracing::error!(
                        panic = %msg,
                        "COM worker panic captured; shutting down worker loop cleanly \
                         (see ComWorker::captured_panic)"
                    );
                    break;
                }
            }

            tracing::debug!("COM worker thread exiting cleanly");
        });

        init_rx
            .recv()
            .map_err(|_| OpcError::Internal("COM worker thread panicked during init".into()))??;

        tracing::debug!("COM worker thread started");

        // P0-1 step D: spawn the health monitor only after init succeeds, so it never
        // fires into an already-dead worker.
        //
        // SAFETY: `tx.clone()` is the one intentional clone of the request sender handed
        // to another thread. It is safe because `Drop` joins this monitor thread (which
        // drops the clone) *before* taking `self.sender`, so the worker's `blocking_recv`
        // still observes `None` and exits.
        let monitor_handle = spawn_health_monitor(
            tx.clone(),
            Arc::clone(&monitor_registry),
            Arc::clone(&monitor_shutdown),
            health,
        );

        Ok(Self {
            sender: Some(tx),
            handle: Some(handle),
            last_panic,
            monitor_registry,
            monitor_handle: Some(monitor_handle),
            monitor_shutdown,
            _phantom: std::marker::PhantomData,
        })
    }

    pub async fn send_request<F, R>(&self, req_builder: F) -> OpcResult<R>
    where
        F: FnOnce(oneshot::Sender<OpcResult<R>>) -> ComRequest,
    {
        if self
            .handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            tracing::error!("COM worker thread panicked or exited unexpectedly");
            return Err(OpcError::Internal("COM worker thread panicked".into()));
        }

        let (tx, rx) = oneshot::channel();
        let req = req_builder(tx);

        let Some(sender) = self.sender.as_ref() else {
            return Err(OpcError::Internal("COM worker already shut down".into()));
        };
        sender
            .send(req)
            .await
            .map_err(|_| OpcError::Internal("COM worker channel closed (worker stopped)".into()))?;

        rx.await
            .map_err(|_| OpcError::Internal("COM worker shut down during request".into()))?
    }

    /// Returns the last panic message captured from the worker thread, if any.
    ///
    /// A `Some` value means the worker previously panicked and has shut down —
    /// useful for health checks and incident root-cause. Populated by the
    /// `catch_unwind` boundary in the worker loop (and the `Drop` join fallback).
    pub fn captured_panic(&self) -> Option<String> {
        self.last_panic.lock().ok().and_then(|guard| guard.clone())
    }

    #[allow(clippy::too_many_lines)]
    fn dispatch_with_retry<F, R>(
        cache: &mut HashMap<String, C::Server>,
        connector: &Arc<C>,
        server_name: &str,
        operation: F,
    ) -> OpcResult<R>
    where
        F: Fn(&C::Server) -> OpcResult<R>,
    {
        const MAX_RECONNECT_ATTEMPTS: u32 = 3;
        let mut last_connection_error: Option<OpcError> = None;
        for attempt in 0..=MAX_RECONNECT_ATTEMPTS {
            let server_ref = match cache.entry(server_name.to_string()) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    tracing::trace!(server = %server_name, "Cache hit");
                    e.into_mut()
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    tracing::debug!(server = %server_name, "Cache miss, connecting");
                    let srv = connector.connect(server_name)?;
                    tracing::info!(server = %server_name, "Connection established, added to pool");
                    e.insert(srv)
                }
            };
            match operation(server_ref) {
                Err(e) if is_connection_error(&e) => {
                    last_connection_error = Some(e);
                    tracing::warn!(
                        server = %server_name,
                        attempt,
                        "Connection error; evicting stale connection"
                    );
                    cache.remove(server_name);
                    if attempt < MAX_RECONNECT_ATTEMPTS {
                        // Exponential backoff between reconnect attempts: 50ms, 100ms, 200ms.
                        let backoff_ms = 50u64 << attempt;
                        tracing::debug!(
                            server = %server_name,
                            attempt,
                            backoff_ms,
                            "Backing off before reconnect"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    }
                }
                other => return other,
            }
        }
        Err(last_connection_error.unwrap_or_else(|| {
            OpcError::Internal("dispatch_with_retry exhausted without executing operation".into())
        }))
    }

    #[allow(clippy::too_many_lines)]
    fn handle_read(
        server_name: &str,
        tag_ids: &[String],
        opc_server: &C::Server,
    ) -> OpcResult<Vec<TagValue>> {
        let span = tracing::info_span!(
            "opc.read_tag_values",
            server = %server_name,
            tag_count = tag_ids.len()
        );
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-read",
            true,
            1000,
            server_handle,
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = tag_ids
            .iter()
            .map(|tag_id| tag_id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();

        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;

        // RemoteArray::len() returns u32; tag_ids.len() returns usize.
        if results.len() as usize != tag_ids.len() || errors.len() as usize != tag_ids.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        let mut tag_values: Vec<TagValue> = tag_ids
            .iter()
            .map(|tag_id| TagValue {
                tag_id: tag_id.clone(),
                value: "Error".to_string(),
                quality: "Bad — not added to group".to_string(),
                timestamp: String::new(),
            })
            .collect();

        let mut server_handles: Vec<ItemHandle> = Vec::new();
        let mut valid_indices = Vec::new();

        for (idx, (item_result, error)) in results
            .as_slice()
            .iter()
            .zip(errors.as_slice().iter())
            .enumerate()
        {
            if error.is_ok() {
                server_handles.push(ItemHandle(item_result.hServer));
                valid_indices.push(idx);
            } else {
                let hint = format_hresult(*error);
                tracing::warn!(
                    tag = %tag_ids[idx],
                    error = %hint,
                    "read_tag_values: add_items rejected tag"
                );
                tag_values[idx].quality = format!("Bad — {hint}");
            }
        }

        if server_handles.is_empty() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Ok(tag_values);
        }

        let (mut item_states, read_errors) = group.read(OPC_DS_DEVICE, &server_handles)?;
        let item_states_slice = item_states.as_slice();
        let read_errors_slice = read_errors.as_slice();

        for (i, idx) in valid_indices.iter().enumerate() {
            let state = &item_states_slice[i];
            let read_error = &read_errors_slice[i];

            let (value_str, quality_str) = if read_error.is_ok() {
                (
                    variant_to_string(&state.vDataValue),
                    quality_to_string(state.wQuality),
                )
            } else {
                let full_msg = format_hresult(*read_error);
                tracing::warn!(
                    tag = %tag_ids[*idx],
                    error = ?read_error,
                    hint = %full_msg,
                    "read_tag_values: per-item read error"
                );
                ("Error".to_string(), format!("Bad — {full_msg}"))
            };

            tag_values[*idx] = TagValue {
                tag_id: tag_ids[*idx].clone(),
                value: value_str,
                quality: quality_str,
                timestamp: filetime_to_string(state.ftTimeStamp),
            };
        }

        // Release VARIANT resources in the COM-allocated item_states array before it drops
        // (RemotePointer::drop only frees the array buffer, not individual VARIANTs).
        clear_item_states(&mut item_states);

        tracing::info!(
            count = tag_values.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "read_tag_values completed"
        );
        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "read_tag_values", "Failed to remove OPC group during cleanup");
        }
        Ok(tag_values)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_read_max_age(
        server_name: &str,
        tag_ids: &[String],
        max_age_ms: u32,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<TagValue>> {
        let span = tracing::info_span!(
            "opc.read_max_age",
            server = %server_name,
            tag_count = tag_ids.len(),
            max_age_ms
        );
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-read-max-age",
            true,
            1000,
            server_handle,
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = tag_ids
            .iter()
            .map(|id| id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();
        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;
        if results.len() as usize != tag_ids.len() || errors.len() as usize != tag_ids.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "read_max_age", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        let mut handles: Vec<ItemHandle> = Vec::new();
        let mut valid_tag_ids: Vec<String> = Vec::new();
        for (idx, (item_result, error)) in results
            .as_slice()
            .iter()
            .zip(errors.as_slice().iter())
            .enumerate()
        {
            if error.is_ok() {
                handles.push(ItemHandle(item_result.hServer));
                valid_tag_ids.push(tag_ids[idx].clone());
            } else {
                let hint = format_hresult(*error);
                tracing::warn!(
                    tag = %tag_ids[idx],
                    error = %hint,
                    "read_max_age: add_items rejected tag"
                );
            }
        }

        let tag_values = if handles.is_empty() {
            Vec::new()
        } else {
            group.read_max_age(&handles, max_age_ms, &valid_tag_ids)?
        };

        tracing::info!(
            count = tag_values.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "read_max_age completed"
        );
        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "read_max_age", "Failed to remove OPC group during cleanup");
        }
        Ok(tag_values)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_write_vqt(
        server_name: &str,
        tag_id: &str,
        value: &OpcValue,
        quality: Option<u16>,
        timestamp: Option<std::time::SystemTime>,
        opc_server: &C::Server,
    ) -> OpcResult<WriteResult> {
        use crate::opc_da::com_utils::TryToNative as _;
        let span = tracing::info_span!("opc.write_vqt", server = %server_name, tag = %tag_id);
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-write-vqt",
            true,
            1000,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let mut item_id_wide: Vec<u16> = tag_id.encode_utf16().chain(std::iter::once(0)).collect();
        let item_def = tagOPCITEMDEF {
            szAccessPath: windows::core::PWSTR::null(),
            szItemID: windows::core::PWSTR(item_id_wide.as_mut_ptr()),
            bActive: windows::Win32::Foundation::TRUE,
            hClient: 0,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
            vtRequestedDataType: 0,
            wReserved: 0,
        };

        let (results, errors) = group.add_items(&[item_def])?;
        let item_res = results
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item results".to_string()))?;
        let item_err = errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item errors".to_string()))?;

        if item_err.is_err() {
            let msg = format!("Failed to add tag: {}", format_hresult(*item_err));
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_vqt", "Failed to remove OPC group during cleanup");
            }
            return Ok(WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(msg),
            });
        }

        let item_handle = ItemHandle(item_res.hServer);
        let variant = opc_value_to_variant(value);
        let ft_time_stamp = match &timestamp {
            Some(t) => t.try_to_native()?,
            None => windows::Win32::Foundation::FILETIME::default(),
        };
        let mut vqt = crate::bindings::da::tagOPCITEMVQT {
            vDataValue: variant,
            bQualitySpecified: windows::core::BOOL::from(quality.is_some()),
            wQuality: quality.unwrap_or(0),
            wReserved: 0,
            bTimeStampSpecified: windows::core::BOOL::from(timestamp.is_some()),
            dwReserved: 0,
            ftTimeStamp: ft_time_stamp,
        };

        let write_errors = group.write_vqt(&[item_handle], &[vqt.clone()])?;
        // SAFETY: the server has consumed/copied the VARIANT; VariantClear releases the
        // local BSTR (for OpcValue::String) to prevent a per-write leak.
        unsafe {
            let _ = VariantClear(&raw mut vqt.vDataValue);
        }
        let write_err = write_errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty write errors".to_string()))?;

        let write_result = if write_err.is_ok() {
            tracing::info!(
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_vqt completed"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: true,
                error: None,
            }
        } else {
            let msg = format_hresult(*write_err);
            tracing::warn!(error = %msg, "write_vqt: server rejected write");
            WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(msg),
            }
        };

        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "write_vqt", "Failed to remove OPC group during cleanup");
        }
        Ok(write_result)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_write(
        server_name: &str,
        tag_id: &str,
        value: &OpcValue,
        opc_server: &C::Server,
    ) -> OpcResult<WriteResult> {
        let span = tracing::info_span!(
            "opc.write_tag_value",
            server = %server_name,
            tag = %tag_id
        );
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-write",
            true,
            1000,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let mut item_id_wide: Vec<u16> = tag_id.encode_utf16().chain(std::iter::once(0)).collect();
        let item_def = tagOPCITEMDEF {
            szAccessPath: windows::core::PWSTR::null(),
            szItemID: windows::core::PWSTR(item_id_wide.as_mut_ptr()),
            bActive: windows::Win32::Foundation::TRUE,
            hClient: 0,
            dwBlobSize: 0,
            pBlob: std::ptr::null_mut(),
            vtRequestedDataType: 0,
            wReserved: 0,
        };

        let (results, errors) = group.add_items(&[item_def])?;
        let item_res = results
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item results".to_string()))?;
        let item_err = errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty item errors".to_string()))?;

        if let Err(e) = item_err.ok() {
            tracing::warn!(error = ?e, "write_tag_value: failed to add tag to group");
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
            }
            return Ok(WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(format!("Failed to add tag: {}", format_hresult(*item_err))),
            });
        }

        let item_handle = ItemHandle(item_res.hServer);
        let mut variant_arr = [opc_value_to_variant(value)];

        let write_errors = group.write(&[item_handle], &variant_arr)?;
        // SAFETY: the server has consumed/copied the VARIANT by now; VariantClear releases
        // the local BSTR (for OpcValue::String) to prevent a per-write leak.
        unsafe {
            let _ = VariantClear(&raw mut variant_arr[0]);
        }
        let write_err = write_errors
            .as_slice()
            .first()
            .ok_or_else(|| OpcError::Internal("Server returned empty write errors".to_string()))?;

        let write_result = if write_err.is_ok() {
            tracing::info!(
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value completed"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: true,
                error: None,
            }
        } else {
            let msg = format_hresult(*write_err);
            tracing::warn!(
                error = %msg,
                elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                "write_tag_value: server rejected write"
            );
            WriteResult {
                tag_id: tag_id.to_string(),
                success: false,
                error: Some(msg),
            }
        };

        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "write_tag_value", "Failed to remove OPC group during cleanup");
        }
        Ok(write_result)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_write_values(
        server_name: &str,
        items: &[(String, OpcValue)],
        opc_server: &C::Server,
    ) -> OpcResult<Vec<WriteResult>> {
        let span = tracing::info_span!(
            "opc.write_tag_values",
            server = %server_name,
            count = items.len()
        );
        let _enter = span.enter();
        let start = std::time::Instant::now();

        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-write-multi",
            true,
            1000,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = items
            .iter()
            .map(|(id, _)| id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();
        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;
        if results.len() as usize != items.len() || errors.len() as usize != items.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        // Default every result to a failure; successful writes overwrite below.
        let mut write_results: Vec<WriteResult> = items
            .iter()
            .map(|(id, _)| WriteResult {
                tag_id: id.clone(),
                success: false,
                error: Some("Failed to add tag".to_string()),
            })
            .collect();

        let mut handles: Vec<ItemHandle> = Vec::new();
        let mut valid_indices: Vec<usize> = Vec::new();
        for (idx, (item_result, error)) in results
            .as_slice()
            .iter()
            .zip(errors.as_slice().iter())
            .enumerate()
        {
            if error.is_ok() {
                handles.push(ItemHandle(item_result.hServer));
                valid_indices.push(idx);
            } else {
                write_results[idx].error =
                    Some(format!("Failed to add tag: {}", format_hresult(*error)));
            }
        }

        if handles.is_empty() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Ok(write_results);
        }

        let mut variants: Vec<_> = valid_indices
            .iter()
            .map(|&i| opc_value_to_variant(&items[i].1))
            .collect();
        let write_errors = group.write(&handles, &variants)?;
        // SAFETY: the server has consumed/copied each VARIANT; release local BSTR resources.
        for v in &mut variants {
            // SAFETY: the server has consumed/copied each VARIANT; VariantClear releases BSTR.
            unsafe {
                let _ = VariantClear(v);
            }
        }
        if write_errors.len() as usize != handles.len() {
            if let Err(e) = opc_server.remove_group(server_handle, true) {
                tracing::warn!(error = ?e, operation = "write_tag_values", "Failed to remove OPC group during cleanup");
            }
            return Err(OpcError::Internal(
                "OPC server returned mismatched write error array size".into(),
            ));
        }

        let write_errors_slice = write_errors.as_slice();
        for (k, &i) in valid_indices.iter().enumerate() {
            let we = &write_errors_slice[k];
            if we.is_ok() {
                write_results[i] = WriteResult {
                    tag_id: items[i].0.clone(),
                    success: true,
                    error: None,
                };
            } else {
                write_results[i] = WriteResult {
                    tag_id: items[i].0.clone(),
                    success: false,
                    error: Some(format_hresult(*we)),
                };
            }
        }

        tracing::info!(
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "write_tag_values completed"
        );
        if let Err(e) = opc_server.remove_group(server_handle, true) {
            tracing::warn!(error = ?e, operation = "write_tag_values", "Failed to remove OPC group during cleanup");
        }
        Ok(write_results)
    }

    fn handle_browse(
        server_name: &str,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        data_type: u16,
        access_rights: u32,
        opc_server: &C::Server,
    ) -> OpcResult<Vec<String>> {
        let span = tracing::info_span!("opc.browse_tags", server = %server_name, max_tags);
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let org = opc_server.query_organization()?;
        let mut tags = Vec::new();

        if org == OPC_NS_FLAT.0 as u32 {
            let string_iter = opc_server.browse_opc_item_ids(
                OPC_LEAF.0 as u32,
                Some(""),
                data_type,
                access_rights,
            )?;
            for tag_res in string_iter {
                if tags.len() >= max_tags {
                    break;
                }
                let tag = tag_res?;
                tags.push(tag.clone());
                if let Ok(mut sink) = tags_sink.lock() {
                    sink.push(tag);
                }
                progress.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            let use_flat = match opc_server.browse_opc_item_ids(
                OPC_FLAT.0 as u32,
                Some(""),
                data_type,
                access_rights,
            ) {
                Ok(mut flat_enum) => match flat_enum.next() {
                    Some(Ok(first_tag)) => {
                        tracing::info!("OPC_FLAT browse supported — using fast flat enumeration");
                        tags.push(first_tag.clone());
                        if let Ok(mut sink) = tags_sink.lock() {
                            sink.push(first_tag);
                        }
                        progress.fetch_add(1, Ordering::Relaxed);

                        for tag_res in flat_enum {
                            if tags.len() >= max_tags {
                                break;
                            }
                            match tag_res {
                                Ok(tag) => {
                                    tags.push(tag.clone());
                                    if let Ok(mut sink) = tags_sink.lock() {
                                        sink.push(tag);
                                    }
                                    progress.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    tracing::warn!(error = ?e, "OPC_FLAT tag iteration error, skipping");
                                }
                            }
                        }
                        true
                    }
                    Some(Err(e)) => {
                        tracing::debug!(error = ?e, "OPC_FLAT first item error, falling back to recursive");
                        false
                    }
                    None => {
                        tracing::debug!("OPC_FLAT returned no items, falling back to recursive");
                        false
                    }
                },
                Err(e) => {
                    tracing::debug!(error = ?e, "OPC_FLAT not supported, falling back to recursive");
                    false
                }
            };

            if !use_flat {
                Self::browse_recursive(
                    opc_server,
                    &mut tags,
                    max_tags,
                    progress,
                    tags_sink,
                    data_type,
                    access_rights,
                    0,
                )?;
            }
        }
        tracing::info!(
            count = tags.len(),
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "browse_tags completed"
        );
        Ok(tags)
    }

    #[allow(clippy::too_many_arguments)]
    fn browse_recursive(
        server: &C::Server,
        tags: &mut Vec<String>,
        max_tags: usize,
        progress: &Arc<AtomicUsize>,
        tags_sink: &Arc<std::sync::Mutex<Vec<String>>>,
        data_type: u16,
        access_rights: u32,
        depth: usize,
    ) -> OpcResult<()> {
        const MAX_DEPTH: usize = 50;
        if depth > MAX_DEPTH || tags.len() >= max_tags {
            if depth > MAX_DEPTH {
                tracing::warn!(depth, "Max browse depth reached, truncating");
            }
            return Ok(());
        }

        let branch_enum =
            server.browse_opc_item_ids(OPC_BRANCH.0 as u32, Some(""), data_type, access_rights)?;

        let branches: Vec<String> = branch_enum
            .filter_map(|r| match r {
                Ok(name) => Some(name),
                Err(e) => {
                    tracing::warn!(error = ?e, "Branch iteration error, skipping");
                    None
                }
            })
            .collect();

        let leaf_enum =
            server.browse_opc_item_ids(OPC_LEAF.0 as u32, Some(""), data_type, access_rights)?;
        for tag_res in leaf_enum {
            if tags.len() >= max_tags {
                return Ok(());
            }
            let browse_name = tag_res?;
            let tag = match server.get_item_id(&browse_name) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        browse_name = %browse_name,
                        error = ?e,
                        "get_item_id failed, using browse name as fallback"
                    );
                    browse_name
                }
            };
            tags.push(tag.clone());
            if let Ok(mut sink) = tags_sink.lock() {
                sink.push(tag);
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }

        for branch in branches {
            if tags.len() >= max_tags {
                return Ok(());
            }
            if let Err(e) = server.change_browse_position(OPC_BROWSE_DOWN.0 as u32, &branch) {
                tracing::warn!(
                    branch = %branch,
                    error = ?e,
                    "Failed to browse down, skipping branch"
                );
                continue;
            }

            if let Err(e) = Self::browse_recursive(
                server,
                tags,
                max_tags,
                progress,
                tags_sink,
                data_type,
                access_rights,
                depth + 1,
            ) {
                tracing::warn!(error = ?e, "browse_recursive error");
            }

            if let Err(e) = server.change_browse_position(OPC_BROWSE_UP.0 as u32, "") {
                tracing::warn!(error = ?e, "Failed to browse up, stopping recursion");
                break;
            }
        }

        Ok(())
    }

    /// Build the `IOPCDataCallback` sink, cast it to `IUnknown`, and advise it on `group`.
    ///
    /// Returns the sink `IUnknown` (the caller must retain it for the subscription's lifetime —
    /// dropping it releases the callback) plus the advise cookie. The caller owns group cleanup
    /// on `Err`: this runs after the group is already added, and a failure (cast or advise)
    /// leaves an empty group that must be torn down via `remove_group`.
    fn build_and_advise_data_callback(
        group: &<C::Server as ConnectedServer>::Group,
        tag_ids: Vec<String>,
        tx: mpsc::Sender<TagValue>,
        last_update: Arc<AtomicU64>,
    ) -> OpcResult<(windows::core::IUnknown, u32)> {
        let sink = DataCallbackSink {
            tag_ids,
            tx: std::sync::Mutex::new(tx),
            last_update,
        };
        // SAFETY: `cast` performs QueryInterface on a locally-owned COM object.
        let sink_callback: crate::bindings::da::IOPCDataCallback = sink.into();
        let sink_iunknown: windows::core::IUnknown = sink_callback.cast()?;
        let cookie = group.advise_data_callback(&sink_iunknown)?;
        Ok((sink_iunknown, cookie))
    }

    /// Create a subscription group on `opc_server`, add the items, build + advise the callback
    /// sink, and set keep-alive. Returns `(server_handle, group, sink, advise_cookie)`. Shared
    /// by `handle_subscribe` (fresh subscribe) and `reconnect_subscription` (rebuild after a
    /// dead server). Any failure after `add_group` cleans up via `remove_group`.
    fn create_group_and_advise(
        opc_server: &C::Server,
        tag_ids: &[String],
        update_rate: u32,
        tx: mpsc::Sender<TagValue>,
        last_update: &Arc<AtomicU64>,
    ) -> OpcResult<(
        GroupHandle,
        <C::Server as ConnectedServer>::Group,
        windows::core::IUnknown,
        u32,
    )> {
        let mut revised_update_rate = 0u32;
        let mut server_handle = GroupHandle::default();
        let group = opc_server.add_group(
            "opc-da-client-subscribe",
            true,
            update_rate,
            GroupHandle(0),
            0,
            0.0,
            0,
            &mut revised_update_rate,
            &mut server_handle,
        )?;

        let item_id_wides: Vec<Vec<u16>> = tag_ids
            .iter()
            .map(|id| id.encode_utf16().chain(std::iter::once(0)).collect())
            .collect();
        let item_defs: Vec<tagOPCITEMDEF> = item_id_wides
            .iter()
            .enumerate()
            .map(|(idx, wide)| tagOPCITEMDEF {
                szAccessPath: windows::core::PWSTR::null(),
                szItemID: windows::core::PWSTR(wide.as_ptr().cast_mut()),
                bActive: windows::Win32::Foundation::TRUE,
                #[allow(clippy::cast_possible_truncation)]
                hClient: idx as u32,
                dwBlobSize: 0,
                pBlob: std::ptr::null_mut(),
                vtRequestedDataType: 0,
                wReserved: 0,
            })
            .collect();

        let (results, errors) = group.add_items(&item_defs)?;
        if results.len() as usize != tag_ids.len() || errors.len() as usize != tag_ids.len() {
            let _ = opc_server.remove_group(server_handle, true);
            return Err(OpcError::Internal(
                "OPC server returned mismatched result array sizes".into(),
            ));
        }

        // Build the sink + advise on the group. Either failing leaves a fresh empty group, so
        // clean it up (P0-1 fix ①: a cast failure previously leaked the group).
        let (sink_iunknown, cookie) = match Self::build_and_advise_data_callback(
            &group,
            tag_ids.to_vec(),
            tx,
            Arc::clone(last_update),
        ) {
            Ok((iu, c)) => (iu, c),
            Err(e) => {
                let _ = opc_server.remove_group(server_handle, true);
                return Err(e);
            }
        };

        // P0-1 step D-6: best-effort keep-alive so a live DA 3.0 server periodically refreshes
        // last_update (via dwcount=0 OnDataChange). DA 2.0 returns NotImplemented — ignored.
        if let Err(e) = group.set_keep_alive(update_rate) {
            tracing::debug!(
                error = ?e,
                "set_keep_alive unavailable; relying on min_timeout"
            );
        }

        Ok((server_handle, group, sink_iunknown, cookie))
    }

    fn handle_subscribe(
        server_name: &str,
        tag_ids: &[String],
        update_rate: u32,
        opc_server: &C::Server,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
        registry: &Arc<Mutex<HashMap<u32, MonitorEntry>>>,
    ) -> OpcResult<SubscriptionHandle> {
        let span = tracing::info_span!(
            "opc.subscribe",
            server = %server_name,
            count = tag_ids.len(),
            update_rate
        );
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let (tx, rx) = mpsc::channel(256);
        // P0-1 step E: separate error channel for subscription-level failures (e.g. rebuild
        // failed after a dead callback). `error_tx` stays in the entry; `errors` goes to client.
        let (error_tx, errors) = mpsc::channel(8);
        // P0-1: liveness timestamp stamped by each OnDataChange; shared with the health
        // monitor to detect a silently-dead callback and trigger rebuild.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        let last_update = Arc::new(AtomicU64::new(now_ms));

        // add_group + add_items + build sink + advise + keep-alive (shared with rebuild's
        // reconnect path). Cleans up the group on any failure.
        let (server_handle, group, sink_iunknown, cookie) = Self::create_group_and_advise(
            opc_server,
            tag_ids,
            update_rate,
            tx.clone(),
            &last_update,
        )?;

        // P0-1 step D: register for health monitoring. Uses the same `last_update` Arc as
        // the entry below, so a rebuild's `store` automatically refreshes the monitor's view.
        if let Ok(mut guard) = registry.lock() {
            guard.insert(
                cookie,
                MonitorEntry {
                    last_update: Arc::clone(&last_update),
                    update_rate_ms: update_rate,
                    pending: None,
                    consecutive_failures: 0,
                    next_retry_after_ms: 0,
                },
            );
        }

        subscriptions.insert(
            cookie,
            SubscriptionEntry {
                server_name: server_name.to_string(),
                server_handle,
                group,
                sink: sink_iunknown,
                last_update,
                tx,
                com_cookie: cookie,
                tag_ids: tag_ids.to_vec(),
                error_tx,
                update_rate,
            },
        );

        tracing::info!(
            cookie,
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "subscribe established"
        );
        Ok(SubscriptionHandle { cookie, rx, errors })
    }

    fn handle_subscribe_request(
        connector: &Arc<C>,
        cache: &mut HashMap<String, C::Server>,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
        registry: &Arc<Mutex<HashMap<u32, MonitorEntry>>>,
        server_name: &str,
        tag_ids: &[String],
        update_rate: u32,
    ) -> OpcResult<SubscriptionHandle> {
        let srv = match cache.entry(server_name.to_string()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(connector.connect(server_name)?)
            }
        };
        Self::handle_subscribe(
            server_name,
            tag_ids,
            update_rate,
            srv,
            subscriptions,
            registry,
        )
    }

    fn handle_unsubscribe(
        cookie: u32,
        cache: &HashMap<String, C::Server>,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
        registry: &Arc<Mutex<HashMap<u32, MonitorEntry>>>,
    ) -> OpcResult<()> {
        let span = tracing::info_span!("opc.unsubscribe", cookie);
        let _enter = span.enter();

        let Some(entry) = subscriptions.remove(&cookie) else {
            return Err(OpcError::InvalidState(format!(
                "unknown subscription cookie {cookie}"
            )));
        };
        // P0-1 step D: stop monitoring this subscription.
        if let Ok(mut guard) = registry.lock() {
            guard.remove(&cookie);
        }
        // Unadvise first (group still alive), then remove the group. Use the tracked COM
        // cookie, which may differ from the client cookie after a P0-1 rebuild re-advised.
        if let Err(e) = entry.group.unadvise_data_callback(entry.com_cookie) {
            tracing::warn!(error = ?e, "unsubscribe: unadvise failed");
        }
        if let Some(server) = cache.get(&entry.server_name)
            && let Err(e) = server.remove_group(entry.server_handle, true)
        {
            tracing::warn!(error = ?e, "unsubscribe: remove_group failed");
        }
        // entry drops here → sink IUnknown releases.
        Ok(())
    }

    /// Rebuild a subscription's callback sink after a detected RPC drop (P0-1 step C).
    ///
    /// Unadvises the stale sink, advises a fresh one cloned from the same mpsc channel, and
    /// updates the tracked COM cookie — all while keeping the client `rx` open (the entry
    /// holds the original `tx`; each sink only holds a clone).
    fn handle_rebuild_subscription(
        cookie: u32,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
        cache: &mut HashMap<String, C::Server>,
        connector: &Arc<C>,
    ) -> OpcResult<()> {
        let span = tracing::info_span!("opc.rebuild_subscription", cookie);
        let _enter = span.enter();

        // Phase 1: lightweight re-advise on the existing group (server alive, callback dead).
        match Self::readvise_existing(cookie, subscriptions) {
            Ok(()) => Ok(()),
            Err(e) if is_connection_error(&e) => {
                // Phase 2: server unreachable (e.g. process killed) → full reconnect.
                tracing::warn!(error = ?e, "rebuild: server unreachable; full reconnect");
                Self::reconnect_subscription(cookie, subscriptions, cache, connector)
            }
            Err(e) => Err(e), // non-connection error: report, don't reconnect
        }
    }

    /// Lightweight rebuild (P0-1 original): unadvise the stale sink + re-advise a fresh sink on
    /// the existing group. Fixes "callback dead, server alive". Does not touch the connection.
    fn readvise_existing(
        cookie: u32,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
    ) -> OpcResult<()> {
        let span = tracing::info_span!("opc.rebuild_readvise", cookie);
        let _enter = span.enter();
        let start = std::time::Instant::now();

        let Some(entry) = subscriptions.get_mut(&cookie) else {
            return Err(OpcError::InvalidState(format!(
                "rebuild: unknown subscription cookie {cookie}"
            )));
        };

        // Unadvise the stale sink first — the group still owns the items, only the callback is
        // dead. A failure here (server already dropped the connection point) is non-fatal.
        if let Err(e) = entry.group.unadvise_data_callback(entry.com_cookie) {
            tracing::warn!(error = ?e, "rebuild: unadvise of stale sink failed");
        }

        // Fresh sink reusing the SAME channel: clone the entry's original tx so the client rx
        // stays open, and reset the liveness clock for the new callback.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        entry.last_update.store(now_ms, Ordering::Relaxed);
        let fresh_sink = DataCallbackSink {
            tag_ids: entry.tag_ids.clone(),
            tx: std::sync::Mutex::new(entry.tx.clone()),
            last_update: Arc::clone(&entry.last_update),
        };
        // SAFETY: `cast` performs QueryInterface on a locally-owned COM object.
        let fresh_callback: crate::bindings::da::IOPCDataCallback = fresh_sink.into();
        let fresh_iunknown: windows::core::IUnknown = fresh_callback.cast()?;
        let new_cookie = entry.group.advise_data_callback(&fresh_iunknown)?;

        // Swap the sink (dropping the old IUnknown releases the old callback; its tx was only a
        // clone, so the channel survives) and record the new COM cookie.
        entry.sink = fresh_iunknown;
        entry.com_cookie = new_cookie;

        tracing::info!(
            new_cookie,
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "subscription rebuilt (lightweight re-advise)"
        );
        Ok(())
    }

    /// Full reconnect after a dead server (P0-1 增强): evict the stale server, reconnect
    /// (DCOM/SCM relaunches it), and re-create group/items/sink with the SAME client `tx` (rx
    /// stays open). Fixes "server process dead".
    fn reconnect_subscription(
        cookie: u32,
        subscriptions: &mut HashMap<u32, SubscriptionEntry<C>>,
        cache: &mut HashMap<String, C::Server>,
        connector: &Arc<C>,
    ) -> OpcResult<()> {
        let span = tracing::info_span!("opc.rebuild_reconnect", cookie);
        let _enter = span.enter();
        let start = std::time::Instant::now();

        // Clone rebuild materials, then release the entry borrow before connecting (avoid
        // holding &mut entry across connector.connect / create_group_and_advise).
        let (server_name, tag_ids, update_rate, tx_clone, last_update) = {
            let Some(entry) = subscriptions.get(&cookie) else {
                return Err(OpcError::InvalidState(format!(
                    "reconnect: unknown subscription cookie {cookie}"
                )));
            };
            (
                entry.server_name.clone(),
                entry.tag_ids.clone(),
                entry.update_rate,
                entry.tx.clone(),
                Arc::clone(&entry.last_update),
            )
        };

        // Reconnect: evict the stale server and create a fresh one (DCOM/SCM relaunches it).
        cache.remove(&server_name);
        let new_server = connector.connect(&server_name)?;

        // Re-create group/items/sink on the new server (reuses the subscribe path).
        let (server_handle, group, sink_iunknown, com_cookie) = Self::create_group_and_advise(
            &new_server,
            &tag_ids,
            update_rate,
            tx_clone,
            &last_update,
        )?;

        // Pool the new server for subsequent read/write ops.
        cache.insert(server_name, new_server);

        // Swap into the entry (old group drops, releasing its COM interfaces; the channel
        // survives because the new sink cloned the entry's original tx). Worker is
        // single-threaded, so the entry cannot be unsubscribed mid-reconnect.
        let entry = subscriptions.get_mut(&cookie).ok_or_else(|| {
            OpcError::InvalidState(format!("reconnect: subscription {cookie} vanished"))
        })?;
        entry.server_handle = server_handle;
        entry.group = group;
        entry.sink = sink_iunknown;
        entry.com_cookie = com_cookie;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        entry.last_update.store(now_ms, Ordering::Relaxed);

        tracing::info!(
            new_cookie = com_cookie,
            elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "subscription rebuilt (full reconnect)"
        );
        Ok(())
    }

    fn handle_subscribe_shutdown(
        server_name: &str,
        opc_server: &C::Server,
        shutdown_subs: &mut HashMap<u32, ShutdownEntry>,
    ) -> OpcResult<ShutdownHandle> {
        let span = tracing::info_span!("opc.subscribe_shutdown", server = %server_name);
        let _enter = span.enter();

        let (tx, rx) = mpsc::channel(8);
        let sink = ShutdownSink {
            tx: std::sync::Mutex::new(tx),
        };
        // SAFETY: `cast` performs QueryInterface on a locally-owned COM object.
        let sink_callback: crate::bindings::comn::IOPCShutdown = sink.into();
        let sink_iunknown: windows::core::IUnknown = sink_callback.cast()?;
        let cookie = opc_server.advise_shutdown(&sink_iunknown)?;
        shutdown_subs.insert(
            cookie,
            ShutdownEntry {
                server_name: server_name.to_string(),
                sink: sink_iunknown,
            },
        );
        tracing::info!(cookie, server = %server_name, "shutdown subscription established");
        Ok(ShutdownHandle { cookie, rx })
    }

    fn handle_unsubscribe_shutdown(
        cookie: u32,
        cache: &HashMap<String, C::Server>,
        shutdown_subs: &mut HashMap<u32, ShutdownEntry>,
    ) -> OpcResult<()> {
        let span = tracing::info_span!("opc.unsubscribe_shutdown", cookie);
        let _enter = span.enter();

        let Some(entry) = shutdown_subs.remove(&cookie) else {
            return Err(OpcError::InvalidState(format!(
                "unknown shutdown cookie {cookie}"
            )));
        };
        if let Some(server) = cache.get(&entry.server_name)
            && let Err(e) = server.unadvise_shutdown(cookie)
        {
            tracing::warn!(error = ?e, "unsubscribe_shutdown: unadvise failed");
        }
        Ok(())
    }
}

impl<C: ServerConnector + 'static> Drop for ComWorker<C> {
    fn drop(&mut self) {
        tracing::debug!("ComWorker dropping — shutting down health monitor, then worker");
        // P0-1 step D: signal + join the health monitor FIRST so its `tx` clone is
        // released. Otherwise that clone keeps the request channel open, the worker's
        // `blocking_recv` never returns `None`, and the worker join below deadlocks.
        self.monitor_shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.monitor_handle.take()
            && let Err(payload) = handle.join()
        {
            let msg = stringify_panic_payload(&payload);
            tracing::error!(
                panic = %msg,
                "health monitor thread panicked during teardown"
            );
        }
        // Close the request channel (now the last sender) so the worker loop exits.
        self.sender.take();
        // P0-3: join the worker thread for deterministic teardown so cached COM
        // resources are released before drop returns (no orphaned thread still
        // mid-request). On a panic that escaped the catch_unwind boundary, also
        // capture the payload as a fallback to `last_panic`.
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(()) => {}
                Err(payload) => {
                    let msg = stringify_panic_payload(&payload);
                    if let Ok(mut guard) = self.last_panic.lock() {
                        *guard = Some(msg.clone());
                    }
                    tracing::error!(
                        panic = %msg,
                        "COM worker thread panicked outside the catch boundary during teardown"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::single_char_pattern,
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        clippy::ptr_as_ptr,
        clippy::borrow_as_ptr,
        clippy::mixed_attributes_style,
        clippy::unreadable_literal,
        clippy::undocumented_unsafe_blocks,
        clippy::manual_assert
    )]
    use super::*;
    use crate::backend::connector::{
        ConnectedGroup, ConnectedServer, RemoteArray, ServerConnector, StringIterator,
    };
    use crate::bindings::da::{tagOPCDATASOURCE, tagOPCITEMDEF, tagOPCITEMRESULT, tagOPCITEMSTATE};

    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    #[derive(Default)]
    struct MockState {
        connect_count: AtomicUsize,
        should_fail_connect: AtomicBool,
        should_fail_write: AtomicBool,
        should_fail_with_connection_error: AtomicBool,
        should_panic_on_request: AtomicBool,
        /// Simulated slow write latency (ms) to force the worker into a busy state.
        slow_write_ms: AtomicU64,
        /// Incremented when a mock server is dropped (worker thread teardown).
        server_drop_count: AtomicUsize,
        /// P0-1: advise/unadvise call counters + cookie source for subscription rebuild tests.
        advise_count: AtomicUsize,
        unadvise_count: AtomicUsize,
        next_cookie: AtomicUsize,
        /// P0-1 step D-6: counts set_keep_alive invocations (success or failure).
        keep_alive_count: AtomicUsize,
        /// P0-1 step D-6: when set, set_keep_alive returns NotImplemented (simulates DA 2.0,
        /// where IOPCGroupStateMgt2 / keep-alive is unavailable).
        should_fail_keep_alive: AtomicBool,
        /// P0-1 step E: when set, advise_data_callback returns NotImplemented — simulates a
        /// rebuild whose re-advise fails (e.g. remote DCOM sink unreachable / 0x800706BA).
        should_fail_advise: AtomicBool,
        /// Counts `remove_group` calls so a test can assert a subscribe failure cleans up its
        /// freshly-added group instead of leaking it (P0-1 review fix ①).
        remove_group_count: AtomicUsize,
        /// P0-1 增强：前 N 次 `advise_data_callback` 返回 `0x800706BA`（RPC server unavailable），
        /// 模拟死代理（server 进程死），驱动 rebuild 的重连路径测试。每次失败 dec；到 0 走正常 advise。
        advise_fail_remaining: AtomicUsize,
    }

    struct ConfigurableMockConnector {
        state: Arc<MockState>,
    }

    struct ConfigurableMockServer {
        state: Arc<MockState>,
    }

    impl Drop for ConfigurableMockServer {
        fn drop(&mut self) {
            self.state.server_drop_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct ConfigurableMockGroup {
        state: Arc<MockState>,
    }

    impl ConnectedGroup for ConfigurableMockGroup {
        fn add_items(
            &self,
            items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            use windows::Win32::Foundation::S_OK;

            let n = items.len();
            if n == 0 {
                return Ok((RemoteArray::empty(), RemoteArray::empty()));
            }

            let res_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(
                    std::mem::size_of::<tagOPCITEMRESULT>() * n,
                )
            } as *mut tagOPCITEMRESULT;
            let err_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(
                    std::mem::size_of::<windows::core::HRESULT>() * n,
                )
            } as *mut windows::core::HRESULT;
            for i in 0..n {
                // SAFETY: `i < n` and both arrays were allocated for `n` elements.
                unsafe {
                    std::ptr::write(
                        res_ptr.add(i),
                        tagOPCITEMRESULT {
                            hServer: (i as u32) + 1,
                            vtCanonicalDataType: 0,
                            wReserved: 0,
                            dwAccessRights: 1,
                            dwBlobSize: 0,
                            pBlob: std::ptr::null_mut(),
                        },
                    );
                    std::ptr::write(err_ptr.add(i), S_OK);
                }
            }

            Ok((
                RemoteArray::from_mut_ptr(res_ptr, n as u32),
                RemoteArray::from_mut_ptr(err_ptr, n as u32),
            ))
        }

        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Ok((RemoteArray::empty(), RemoteArray::empty()))
        }

        fn write(
            &self,
            server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            let delay = self.state.slow_write_ms.load(Ordering::Relaxed);
            if delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
            if self
                .state
                .should_fail_with_connection_error
                .load(Ordering::Relaxed)
            {
                // RPC server unavailable (0x800706BA) triggers connection eviction
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x800706BA_u32 as i32,
                    )),
                });
            }

            let n = server_handles.len();
            if n == 0 {
                return Ok(RemoteArray::empty());
            }

            let hr = if self.state.should_fail_write.load(Ordering::Relaxed) {
                windows::Win32::Foundation::E_FAIL
            } else {
                windows::Win32::Foundation::S_OK
            };

            let hr_ptr = unsafe {
                windows::Win32::System::Com::CoTaskMemAlloc(
                    std::mem::size_of::<windows::core::HRESULT>() * n,
                )
            } as *mut windows::core::HRESULT;
            for i in 0..n {
                // SAFETY: `i < n` and the array was allocated for `n` elements.
                unsafe {
                    std::ptr::write(hr_ptr.add(i), hr);
                }
            }

            Ok(RemoteArray::from_mut_ptr(hr_ptr, n as u32))
        }

        fn advise_data_callback(&self, _sink: &windows::core::IUnknown) -> OpcResult<u32> {
            // P0-1 增强：模拟死代理（server 进程死）—— 前 N 次 advise 返 RPC 连接错误
            // (0x800706BA)，驱动 rebuild 的重连路径。每次失败 dec；到 0 走正常 advise。
            let remaining = self.state.advise_fail_remaining.load(Ordering::Relaxed);
            if remaining > 0 {
                self.state
                    .advise_fail_remaining
                    .store(remaining - 1, Ordering::Relaxed);
                return Err(OpcError::Com {
                    source: windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x800706BA_u32 as i32,
                    )),
                });
            }
            // P0-1 step E: simulate a rebuild whose re-advise fails (NotImplemented).
            if self.state.should_fail_advise.load(Ordering::Relaxed) {
                return Err(OpcError::NotImplemented(
                    "advise_data_callback failed (simulated)".into(),
                ));
            }
            self.state.advise_count.fetch_add(1, Ordering::Relaxed);
            // P0-1: monotonically increasing cookie so rebuild (re-advise) yields a
            // distinct cookie, mirroring real OPC IConnectionPoint::Advise.
            let cookie = u32::try_from(self.state.next_cookie.fetch_add(1, Ordering::Relaxed))
                .unwrap_or(0)
                + 1;
            Ok(cookie)
        }

        fn unadvise_data_callback(&self, _cookie: u32) -> OpcResult<()> {
            self.state.unadvise_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn set_keep_alive(&self, keep_alive_ms: u32) -> OpcResult<u32> {
            self.state.keep_alive_count.fetch_add(1, Ordering::Relaxed);
            if self.state.should_fail_keep_alive.load(Ordering::Relaxed) {
                Err(OpcError::NotImplemented(
                    "set_keep_alive not supported (simulated DA 2.0)".into(),
                ))
            } else {
                Ok(keep_alive_ms)
            }
        }
    }

    impl ConnectedServer for ConfigurableMockServer {
        type Group = ConfigurableMockGroup;

        fn query_organization(&self) -> OpcResult<u32> {
            Ok(0)
        }

        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }

        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }

        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Ok(String::new())
        }

        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            if self.state.should_panic_on_request.load(Ordering::Relaxed) {
                panic!("Simulated worker panic");
            }
            Ok(ConfigurableMockGroup {
                state: self.state.clone(),
            })
        }

        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            self.state
                .remove_group_count
                .fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    impl ServerConnector for ConfigurableMockConnector {
        type Server = ConfigurableMockServer;

        fn enumerate_servers(&self, _host: &str) -> OpcResult<Vec<String>> {
            if self.state.should_fail_connect.load(Ordering::Relaxed) {
                Err(OpcError::Internal("Server enumeration failed".into()))
            } else {
                Ok(vec!["Mock.Server.1".into()])
            }
        }

        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            if self.state.should_fail_connect.load(Ordering::Relaxed) {
                Err(OpcError::Internal("Connection failed".into()))
            } else {
                self.state.connect_count.fetch_add(1, Ordering::Relaxed);
                Ok(ConfigurableMockServer {
                    state: self.state.clone(),
                })
            }
        }
    }

    struct WorkerMockConnector;
    struct WorkerMockServer;
    struct WorkerMockGroup;

    impl ConnectedGroup for WorkerMockGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn write(
            &self,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            Err(OpcError::NotImplemented("mock".into()))
        }
    }

    impl ConnectedServer for WorkerMockServer {
        type Group = WorkerMockGroup;
        fn query_organization(&self) -> OpcResult<u32> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn browse_opc_item_ids(
            &self,
            _browse_type: u32,
            _filter: Option<&str>,
            _data_type: u16,
            _access_rights: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            Err(OpcError::NotImplemented("mock".into()))
        }
    }

    impl ServerConnector for WorkerMockConnector {
        type Server = WorkerMockServer;
        fn enumerate_servers(&self, _host: &str) -> OpcResult<Vec<String>> {
            Ok(vec!["Mock.Server.1".into()])
        }
        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(WorkerMockServer)
        }
    }

    #[tokio::test]
    async fn test_worker_starts_and_stops() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(WorkerMockConnector)).unwrap()
        })
        .await
        .unwrap();
        drop(worker);
    }

    #[tokio::test]
    async fn test_worker_list_servers() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(WorkerMockConnector)).unwrap()
        })
        .await
        .unwrap();
        let (reply, _rx) = oneshot::channel();
        worker
            .sender
            .as_ref()
            .expect("sender present")
            .send(ComRequest::ListServers {
                host: "localhost".into(),
                reply,
            })
            .await
            .unwrap();
        // Wait for implementation
    }

    struct MismatchedConnector;
    struct MismatchedServer;
    struct MismatchedGroup;

    impl ConnectedGroup for MismatchedGroup {
        fn add_items(
            &self,
            _items: &[tagOPCITEMDEF],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMRESULT>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Ok((RemoteArray::empty(), RemoteArray::empty()))
        }
        fn read(
            &self,
            _source: tagOPCDATASOURCE,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
        ) -> OpcResult<(
            RemoteArray<tagOPCITEMSTATE>,
            RemoteArray<windows::core::HRESULT>,
        )> {
            Ok((RemoteArray::empty(), RemoteArray::empty()))
        }
        fn write(
            &self,
            _server_handles: &[crate::opc_da::typedefs::ItemHandle],
            _values: &[windows::Win32::System::Variant::VARIANT],
        ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
            Ok(RemoteArray::empty())
        }
    }

    impl ConnectedServer for MismatchedServer {
        type Group = MismatchedGroup;
        fn query_organization(&self) -> OpcResult<u32> {
            Ok(0)
        }
        fn browse_opc_item_ids(
            &self,
            _b: u32,
            _f: Option<&str>,
            _d: u16,
            _a: u32,
        ) -> OpcResult<StringIterator> {
            Err(OpcError::NotImplemented("mock".into()))
        }
        fn change_browse_position(&self, _direction: u32, _name: &str) -> OpcResult<()> {
            Ok(())
        }
        fn get_item_id(&self, _item_name: &str) -> OpcResult<String> {
            Ok(String::new())
        }
        fn add_group(
            &self,
            _name: &str,
            _active: bool,
            _update_rate: u32,
            _client_handle: crate::opc_da::typedefs::GroupHandle,
            _time_bias: i32,
            _percent_deadband: f32,
            _locale_id: u32,
            _revised_update_rate: &mut u32,
            _server_handle: &mut crate::opc_da::typedefs::GroupHandle,
        ) -> OpcResult<Self::Group> {
            Ok(MismatchedGroup)
        }
        fn remove_group(
            &self,
            _server_group: crate::opc_da::typedefs::GroupHandle,
            _force: bool,
        ) -> OpcResult<()> {
            Ok(())
        }
    }

    impl ServerConnector for MismatchedConnector {
        type Server = MismatchedServer;
        fn enumerate_servers(&self, _host: &str) -> OpcResult<Vec<String>> {
            Ok(vec![])
        }
        fn connect(&self, _server_name: &str) -> OpcResult<Self::Server> {
            Ok(MismatchedServer)
        }
    }

    #[tokio::test]
    async fn test_worker_read_tag_values_mismatched_lengths() {
        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(MismatchedConnector)).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: "MockServer".to_string(),
                tag_ids: vec!["Tag1".to_string(), "Tag2".to_string()],
                reply,
            })
            .await;

        assert!(
            result.is_err(),
            "Expected read to fail due to mismatched lengths"
        );
        if let Err(OpcError::Internal(msg)) = result {
            assert!(msg.contains("mismatched result array sizes"));
        } else {
            panic!("Expected OpcError::Internal, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_worker_write_tag_value() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Random.Int4".to_string(),
                value: OpcValue::Int(42),
                reply,
            })
            .await
            .expect("Request should succeed");

        assert_eq!(result.tag_id, "Random.Int4");
        assert!(result.success, "Write should be successful");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_worker_write_tag_values_multi() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let items = vec![
            ("Tag1".to_string(), OpcValue::Int(1)),
            ("Tag2".to_string(), OpcValue::Int(2)),
            ("Tag3".to_string(), OpcValue::Int(3)),
        ];

        let results = worker
            .send_request(|reply| ComRequest::WriteTagValues {
                server: "Mock.Server.1".to_string(),
                items,
                reply,
            })
            .await
            .expect("multi-write should succeed");

        assert_eq!(results.len(), 3);
        assert!(
            results.iter().all(|r| r.success),
            "all writes should succeed: {results:?}"
        );
        assert_eq!(results[0].tag_id, "Tag1");
        assert_eq!(results[2].tag_id, "Tag3");
    }

    #[tokio::test]
    async fn test_connection_cache_reuse() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await
            .unwrap();

        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag2".to_string(),
                value: OpcValue::Int(2),
                reply,
            })
            .await
            .unwrap();

        assert_eq!(
            state.connect_count.load(Ordering::Relaxed),
            1,
            "Server connection should be cached and reused"
        );
    }

    #[tokio::test]
    async fn test_stale_connection_eviction() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // Initial connect
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await
            .unwrap();

        assert_eq!(state.connect_count.load(Ordering::Relaxed), 1);

        // Enable connection error flag to trigger eviction on next operation
        state
            .should_fail_with_connection_error
            .store(true, Ordering::Relaxed);

        // Next request triggers eviction and reconnect attempt
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag2".to_string(),
                value: OpcValue::Int(2),
                reply,
            })
            .await;

        assert!(
            state.connect_count.load(Ordering::Relaxed) >= 2,
            "Stale connection should be evicted and reconnected at least once: {}",
            state.connect_count.load(Ordering::Relaxed)
        );
    }

    #[tokio::test]
    async fn test_worker_panic_propagation() {
        let state = Arc::new(MockState::default());
        state.should_panic_on_request.store(true, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await;

        assert!(result.is_err());
        if let Err(OpcError::Internal(msg)) = result {
            assert!(
                msg.contains("shut down")
                    || msg.contains("channel closed")
                    || msg.contains("panicked"),
                "Expected worker termination message, got: {}",
                msg
            );
        } else {
            panic!("Expected OpcError::Internal, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_worker_panic_payload_is_captured() {
        // P0-4: a panic inside the worker loop must be captured into a shared,
        // cross-thread observable cell — not silently dropped — so production
        // incidents retain a root-cause message.
        let state = Arc::new(MockState::default());
        state.should_panic_on_request.store(true, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // `add_group` panics inside the worker thread.
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await;

        // The reply (oneshot) drops as the handler unwinds, so `send_request` returns
        // before the worker writes `worker_last_panic` (which happens just after
        // `catch_unwind` returns). Poll briefly until the payload is observable — without
        // this the assertion races the worker thread and fails intermittently.
        let mut captured = worker.captured_panic();
        for _ in 0..100 {
            if captured.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
            captured = worker.captured_panic();
        }
        assert!(
            captured.is_some(),
            "worker panic payload must be captured, not silently lost"
        );
        assert!(
            captured
                .as_deref()
                .is_some_and(|m| m.contains("Simulated worker panic")),
            "captured payload must preserve the original panic message, got: {captured:?}"
        );
    }

    #[tokio::test]
    async fn test_drop_joins_worker_until_thread_exits() {
        // P0-3: ComWorker::drop must join the worker thread so cached COM
        // resources are released before drop returns — no detached worker
        // still mid-request.
        let state = Arc::new(MockState::default());
        state.slow_write_ms.store(100, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // Drive the worker into a slow in-flight write via a cloned sender,
        // without borrowing `worker` (so we can drop it next).
        let sender = worker.sender.clone().expect("worker sender present");
        let (tx, _rx) = oneshot::channel();
        sender
            .send(ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "Tag1".to_string(),
                value: OpcValue::Int(1),
                reply: tx,
            })
            .await
            .unwrap();
        // Release the cloned sender so the channel closes once `worker.sender`
        // is taken in Drop — otherwise the worker blocks on `blocking_recv`
        // forever and `join()` deadlocks.
        drop(sender);
        // Let the worker enter the slow write.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        drop(worker);

        assert!(
            state.server_drop_count.load(Ordering::Relaxed) >= 1,
            "drop must join the worker until it exits and releases the cached server"
        );
    }

    #[tokio::test]
    async fn test_drop_during_active_request() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // Dropping worker handle closes channel gracefully
        drop(worker);
    }

    #[tokio::test]
    async fn test_worker_subscribes_via_mock() {
        // P0-1 prerequisite: the mock must support advise so subscription (and the
        // upcoming rebuild path) can be exercised in unit tests. The trait default
        // returns NotImplemented, so this fails until the mock overrides it.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["Tag1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed once mock supports advise");

        assert!(handle.cookie > 0, "subscription should receive a cookie");
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            1,
            "advise_data_callback should be called exactly once"
        );
        assert_eq!(
            state.unadvise_count.load(Ordering::Relaxed),
            0,
            "no unadvise during a fresh subscribe"
        );
    }

    #[tokio::test]
    async fn test_rebuild_subscription_keeps_rx_open() {
        // P0-1 step B+C: rebuilding a subscription's sink (unadvise old → re-advise new)
        // after an RPC drop must NOT close the client rx. The entry keeps the original
        // mpsc tx; the sink only holds a clone, so releasing the old sink leaves the
        // channel open for the fresh sink's clone to keep feeding.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["Tag1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");

        let cookie = handle.cookie;
        let mut rx = handle.rx;

        worker
            .send_request(|reply| ComRequest::RebuildSubscription { cookie, reply })
            .await
            .expect("rebuild should succeed");

        // Step C: rebuild unadvised the stale sink and re-advised a fresh one.
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            2,
            "rebuild must re-advise a fresh sink"
        );
        assert_eq!(
            state.unadvise_count.load(Ordering::Relaxed),
            1,
            "rebuild must unadvise the old sink"
        );

        // Step B: rx stays open — releasing the old sink (its tx is only a clone) does not
        // close the channel while the entry holds the original tx. An open, empty channel
        // makes recv() await; a closed one returns None immediately.
        let closed = tokio::time::timeout(std::time::Duration::from_millis(150), rx.recv())
            .await
            .is_ok_and(|opt| opt.is_none());
        assert!(
            !closed,
            "rx must stay open after rebuild (entry holds the original tx)"
        );

        drop(worker);
    }

    #[tokio::test]
    async fn test_start_with_health_constructs_and_drops_cleanly() {
        // P0-1 D-1: start_with_health constructs a worker + health monitor thread whose
        // Drop joins the monitor *before* closing the worker channel. If the join order
        // were wrong, `drop(worker)` would deadlock (the monitor's tx clone keeps the
        // channel open → worker blocking_recv never returns None → worker join hangs).
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert!(
            worker.sender.is_some(),
            "worker should hold a request sender"
        );
        // Drop must return — proves the monitor join precedes channel close.
        drop(worker);
    }

    #[tokio::test]
    async fn test_subscribe_registers_in_monitor_registry() {
        // P0-1 D-2: subscribe registers the cookie in the shared health-monitor registry;
        // unsubscribe removes it. (The monitor scan itself lands in D-3.)
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");

        let cookie = handle.cookie;
        assert!(
            worker
                .monitor_registry
                .lock()
                .is_ok_and(|g| g.contains_key(&cookie)),
            "subscribe must register the cookie in the monitor registry"
        );

        worker
            .send_request(|reply| ComRequest::Unsubscribe { cookie, reply })
            .await
            .expect("unsubscribe should succeed");
        assert!(
            !worker
                .monitor_registry
                .lock()
                .map_or(true, |g| g.contains_key(&cookie)),
            "unsubscribe must remove the cookie from the monitor registry"
        );
    }

    #[tokio::test]
    async fn test_health_monitor_stale_triggers_rebuild() {
        // P0-1 D-3: when a subscription's last_update goes stale (the mock never pushes
        // OnDataChange, so last_update freezes at subscribe time), the health monitor must
        // fire a RebuildSubscription → advise_count climbs 1→2 and the stale sink is unadvised.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();

        // update_rate=10 → threshold = max(10*3, 100) = 100ms.
        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 10,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(state.advise_count.load(Ordering::Relaxed), 1);

        // Poll for the monitor-triggered rebuild (advises a fresh sink). ~2s ceiling.
        let mut rebuilt = false;
        for _ in 0..200 {
            if state.advise_count.load(Ordering::Relaxed) >= 2 {
                rebuilt = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            rebuilt,
            "health monitor must trigger a rebuild when a subscription goes stale"
        );
        assert_eq!(
            state.unadvise_count.load(Ordering::Relaxed),
            1,
            "rebuild must unadvise the stale sink"
        );
        drop(worker);
        drop(handle);
    }

    #[tokio::test]
    async fn test_health_monitor_no_premature_rebuild() {
        // P0-1 D-4: the monitor must NOT rebuild before the staleness threshold elapses.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();

        let _handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 10,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(state.advise_count.load(Ordering::Relaxed), 1);

        // Sleep under the 100ms threshold → no rebuild yet.
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            1,
            "monitor must not rebuild before the staleness threshold"
        );
        drop(worker);
    }

    #[tokio::test]
    async fn test_health_monitor_fresh_callback_no_rebuild() {
        // P0-1 D-5: a subscription whose last_update is kept fresh (as if OnDataChange
        // keeps arriving) must NOT be rebuilt, even well past the threshold.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 10,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        let cookie = handle.cookie;

        // Refresh last_update ~every 30ms (simulating OnDataChange) for ~1s — well past
        // the 100ms threshold, yet a live callback must not be rebuilt.
        for _ in 0..33 {
            if let Ok(g) = worker.monitor_registry.lock()
                && let Some(entry) = g.get(&cookie)
            {
                entry.last_update.store(now_ms(), Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            1,
            "a freshly-stamped callback must not be rebuilt"
        );
        drop(worker);
        drop(handle);
    }

    #[tokio::test]
    async fn test_subscribe_invokes_set_keep_alive() {
        // P0-1 D-6: subscribe best-effort calls set_keep_alive so a DA 3.0 server refreshes
        // last_update via keep-alive callbacks (dwcount=0 OnDataChange).
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let _handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(
            state.keep_alive_count.load(Ordering::Relaxed),
            1,
            "subscribe must best-effort invoke set_keep_alive"
        );
    }

    #[tokio::test]
    async fn test_subscribe_survives_set_keep_alive_failure() {
        // P0-1 D-6: if set_keep_alive returns NotImplemented (DA 2.0 without
        // IOPCGroupStateMgt2), subscribe must still succeed — best-effort, not fatal.
        let state = Arc::new(MockState::default());
        state.should_fail_keep_alive.store(true, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await;
        assert!(
            result.is_ok(),
            "subscribe must succeed even if set_keep_alive is unsupported: {result:?}"
        );
        assert_eq!(
            state.keep_alive_count.load(Ordering::Relaxed),
            1,
            "set_keep_alive must still be invoked (and its failure absorbed)"
        );
    }

    #[tokio::test]
    async fn test_subscribe_cleans_up_group_when_sink_creation_fails() {
        // P0-1 review fix ①: when the sink-creation step (cast/advise) fails after the group
        // was added, handle_subscribe must remove the group so it doesn't leak. Observed via the
        // mock's remove_group counter.
        //
        // Note: a `cast::<IUnknown>()` failure is not directly injectable — IUnknown QI on a
        // `#[implement]` object always succeeds — so we drive the shared cleanup path via an
        // advise failure, which after the fix routes through the same cleanup arm as a cast
        // failure would.
        let state = Arc::new(MockState::default());
        state.should_fail_advise.store(true, Ordering::Relaxed);
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await;
        assert!(
            result.is_err(),
            "subscribe must fail when advise fails: {result:?}"
        );
        assert_eq!(
            state.remove_group_count.load(Ordering::Relaxed),
            1,
            "subscribe must remove the group on sink-creation failure (no leak)"
        );
    }

    #[tokio::test]
    async fn test_subscribe_yields_empty_error_channel() {
        // P0-1 E-1: subscribe returns a handle with a separate error channel alongside rx
        // (rx stays pure TagValue). Right after subscribe the error channel is empty.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let mut handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert!(
            matches!(
                handle.errors.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "error channel should be empty right after a fresh subscribe"
        );
    }

    #[tokio::test]
    async fn test_rebuild_failure_surfaces_on_error_channel() {
        // P0-1 E-2: when a rebuild's re-advise fails, the worker must push an OpcError onto the
        // subscription's error channel (not silent). The mock advises succeed at subscribe but
        // fail on the rebuild's re-advise.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let mut handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(state.advise_count.load(Ordering::Relaxed), 1);

        // Make the rebuild's re-advise fail, then trigger a rebuild directly.
        state.should_fail_advise.store(true, Ordering::Relaxed);
        worker
            .send_request(|reply| ComRequest::RebuildSubscription {
                cookie: handle.cookie,
                reply,
            })
            .await
            .expect_err("rebuild should fail when re-advise fails");

        // The failure must surface on the error channel (not be silent).
        let err = tokio::time::timeout(std::time::Duration::from_millis(200), handle.errors.recv())
            .await
            .expect("error channel should receive the rebuild failure")
            .expect("error channel must stay open");
        assert!(
            matches!(err, OpcError::Internal(ref msg) if msg.contains("rebuild failed")),
            "error should describe the rebuild failure, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_rebuild_reconnects_when_server_dead() {
        // P0-1 增强：rebuild 的轻量 re-advise 失败且为连接错误（0x800706BA，server 死）时，
        // 必须重连 server 并重建 group/items/sink，而非只报错。subscribe 成功后设
        // advise_fail_remaining=1 → rebuild 轻量 re-advise 返连接错误 → 触发重连 → 新 server
        // (connect_count 增加) + 新 group advise 成功 → rebuild Ok。
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(state.advise_count.load(Ordering::Relaxed), 1);
        let connect_after_subscribe = state.connect_count.load(Ordering::Relaxed);

        // 下一次 advise（rebuild 的轻量 re-advise）返连接错误 → 触发重连。
        state.advise_fail_remaining.store(1, Ordering::Relaxed);
        worker
            .send_request(|reply| ComRequest::RebuildSubscription {
                cookie: handle.cookie,
                reply,
            })
            .await
            .expect("rebuild should succeed after reconnecting the dead server");

        // 重连：connector.connect 再调一次（新 server）。
        assert!(
            state.connect_count.load(Ordering::Relaxed) > connect_after_subscribe,
            "rebuild must reconnect the server after a connection error"
        );
        // 重连后新 group 的 advise 成功（subscribe=1 + 重连新 advise=2；轻量那次失败不计）。
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            2,
            "rebuild must re-advise on the new group after reconnect"
        );
    }

    #[tokio::test]
    async fn test_rebuild_non_connection_error_does_not_reconnect() {
        // P0-1：re-advise 失败但非连接错误（NotImplemented）时不重连（避免对非断线错误做无谓重连）。
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(connector, HealthMonitorConfig::production()).unwrap()
        })
        .await
        .unwrap();

        let handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 100,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        let connect_after_subscribe = state.connect_count.load(Ordering::Relaxed);

        // should_fail_advise → NotImplemented（非连接错误）。
        state.should_fail_advise.store(true, Ordering::Relaxed);
        let result = worker
            .send_request(|reply| ComRequest::RebuildSubscription {
                cookie: handle.cookie,
                reply,
            })
            .await;
        assert!(
            result.is_err(),
            "rebuild should fail when re-advise returns a non-connection error"
        );
        assert_eq!(
            state.connect_count.load(Ordering::Relaxed),
            connect_after_subscribe,
            "non-connection error must not trigger a reconnect"
        );
    }

    #[tokio::test]
    async fn test_monitor_backoff_on_repeated_rebuild_failure() {
        // P0-1 E-3: repeated rebuild failures trigger exponential backoff. Without backoff the
        // monitor fires roughly every (threshold+period) ≈ 150ms (~13 attempts in 2s); backoff
        // (period*2^n) slows it to a handful. Observed via unadvise_count (ticks once per
        // rebuild attempt — unadvise succeeds even when the subsequent re-advise fails).
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();

        let _handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 10,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        state.should_fail_advise.store(true, Ordering::Relaxed);

        // 2s window: without backoff ~13 attempts; with exponential backoff a handful.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let attempts = state.unadvise_count.load(Ordering::Relaxed);
        assert!(
            attempts <= 7,
            "exponential backoff must slow repeated rebuild failures, got {attempts} in 2s"
        );
    }

    #[tokio::test]
    async fn test_monitor_backoff_resets_on_success() {
        // P0-1 E-4: after backoff from repeated failures, a successful rebuild resets the
        // backoff (consecutive_failures=0) and the monitor resumes — advise_count climbs.
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            ComWorker::start_with_health(
                connector,
                HealthMonitorConfig {
                    period: std::time::Duration::from_millis(50),
                    min_timeout: std::time::Duration::from_millis(100),
                },
            )
            .unwrap()
        })
        .await
        .unwrap();

        let _handle = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 10,
                reply,
            })
            .await
            .expect("subscribe should succeed");
        assert_eq!(state.advise_count.load(Ordering::Relaxed), 1);

        // Phase 1: fail rebuilds for ~500ms → backoff kicks in (advise_count stays at 1).
        state.should_fail_advise.store(true, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            1,
            "failed rebuilds must not increment advise_count"
        );

        // Phase 2: let rebuilds succeed again. After a successful rebuild the backoff window
        // clears and the monitor resumes — advise_count climbs past 1.
        state.should_fail_advise.store(false, Ordering::Relaxed);
        let mut recovered = false;
        for _ in 0..100 {
            // ~5s ceiling
            if state.advise_count.load(Ordering::Relaxed) >= 2 {
                recovered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            recovered,
            "a successful rebuild after backoff must reset and resume (advise_count should climb)"
        );
    }

    #[tokio::test]
    async fn test_worker_init_failure() {
        struct FailingInitConnector;
        impl ServerConnector for FailingInitConnector {
            type Server = ConfigurableMockServer;
            fn enumerate_servers(&self, _host: &str) -> OpcResult<Vec<String>> {
                Err(OpcError::Internal("COM subsystem failed".into()))
            }
            fn connect(&self, _name: &str) -> OpcResult<Self::Server> {
                Err(OpcError::Internal("COM subsystem failed".into()))
            }
        }

        let worker = tokio::task::spawn_blocking(|| {
            ComWorker::start(Arc::new(FailingInitConnector)).unwrap()
        })
        .await
        .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::ListServers {
                host: "localhost".into(),
                reply,
            })
            .await;

        assert!(
            result.is_err(),
            "ListServers request should fail when connector enumeration fails"
        );
    }

    #[tokio::test]
    async fn test_get_server_status_routes_to_default() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::GetServerStatus {
                server: "Mock.Server.1".to_string(),
                reply,
            })
            .await;

        // ConfigurableMockServer inherits ConnectedServer::get_status default impl,
        // which returns NotImplemented — proves the request routes through the
        // worker channel to the server facade (not silently dropped).
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_get_item_properties_routes_to_default() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::GetItemProperties {
                server: "Mock.Server.1".to_string(),
                tag_id: "Random.Int4".to_string(),
                reply,
            })
            .await;

        // ConfigurableMockServer inherits ConnectedServer::get_item_properties default
        // impl (NotImplemented) — proves routing to the server facade.
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_read_max_age_routes_to_group_default() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::ReadMaxAge {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                max_age_ms: 1000,
                reply,
            })
            .await;

        // add_items succeeds on ConfigurableMockGroup, then read_max_age hits the
        // ConnectedGroup default (NotImplemented) — proves routing into the group facade.
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_write_vqt_routes_to_group_default() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::WriteTagValueVqt {
                server: "Mock.Server.1".to_string(),
                tag_id: "T1".to_string(),
                value: OpcValue::Int(7),
                quality: Some(0xC0),
                timestamp: None,
                reply,
            })
            .await;

        // add_items succeeds, then write_vqt hits the ConnectedGroup default (NotImplemented).
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_get_error_string_routes_to_default() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::GetErrorString {
                server: "Mock.Server.1".to_string(),
                hresult: 0,
                reply,
            })
            .await;

        // ConfigurableMockServer inherits ConnectedServer::get_error_string default (NotImplemented).
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_explicit_reconnect_re_establishes_connection() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        // initial op connects once
        let _ = worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: "Mock.Server.1".to_string(),
                tag_id: "T1".to_string(),
                value: OpcValue::Int(1),
                reply,
            })
            .await
            .unwrap();
        assert_eq!(state.connect_count.load(Ordering::Relaxed), 1);

        // explicit reconnect forces a fresh connect (count -> 2)
        let r = worker
            .send_request(|reply| ComRequest::Reconnect {
                server: "Mock.Server.1".to_string(),
                reply,
            })
            .await;
        assert!(r.is_ok(), "reconnect should succeed: {r:?}");
        assert_eq!(
            state.connect_count.load(Ordering::Relaxed),
            2,
            "explicit reconnect must re-establish the connection"
        );
    }

    #[tokio::test]
    async fn test_subscribe_routes_to_advise() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector {
            state: state.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::Subscribe {
                server: "Mock.Server.1".to_string(),
                tag_ids: vec!["T1".to_string()],
                update_rate: 1000,
                reply,
            })
            .await;

        // add_group + add_items + advise_data_callback all succeed on the mock
        // (ConfigurableMockGroup overrides advise); the advise counter proves routing into
        // the group facade.
        assert!(result.is_ok(), "subscribe should succeed: {result:?}");
        assert_eq!(
            state.advise_count.load(Ordering::Relaxed),
            1,
            "subscribe must route into group.advise_data_callback"
        );
    }

    #[tokio::test]
    async fn test_subscribe_shutdown_routes_to_advise() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::SubscribeShutdown {
                server: "Mock.Server.1".to_string(),
                reply,
            })
            .await;

        // ConfigurableMockServer inherits ConnectedServer::advise_shutdown default (NotImplemented).
        assert!(matches!(result, Err(OpcError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn test_set_subscription_rate_unknown_cookie() {
        let state = Arc::new(MockState::default());
        let connector = Arc::new(ConfigurableMockConnector { state });
        let worker = tokio::task::spawn_blocking(move || ComWorker::start(connector).unwrap())
            .await
            .unwrap();

        let result = worker
            .send_request(|reply| ComRequest::SetSubscriptionRate {
                cookie: 999,
                update_rate: 500,
                reply,
            })
            .await;

        assert!(
            matches!(result, Err(OpcError::InvalidState(_))),
            "unknown cookie must yield InvalidState, got {result:?}"
        );
    }
}
