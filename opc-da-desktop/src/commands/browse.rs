//! Browse IPC handlers — back the "Add tags" modal.
//!
//! `opc-da-client::OpcProvider::browse_tags` is designed for long-running
//! browse walks: it pushes discoveries into a `tags_sink` as it finds
//! them and returns the final vec. We bridge that to a Tauri
//! `Channel<TagDescriptor>` so the WebView can render progressive
//! results.
//!
//! ## P0 audit fix — drain task no longer infinite-loops on failure
//!
//! The previous version used `progress == sink.len()` as the "browse
//! finished" signal. When the browse call returned an error, both
//! counters stayed at 0 and the drain task looped forever, leaving
//! the IPC handler hung. We now use an explicit `done: AtomicBool`
//! flag that the browse completion code sets in both the success and
//! error paths.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;

use opc_da_client::OpcProvider;

use crate::error::{DesktopError, DesktopResult};
use crate::state::AppState;

/// One leaf tag pushed through the channel during browse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagDescriptor {
    /// Fully-qualified tag path (e.g. `"Random.Real4"`).
    pub item_id: String,
}

/// Stream the leaf tags found under the connected server's namespace.
///
/// `max_tags` caps the total result (default 1000). The handler
/// resolves only after the browse has finished (Ok or Err) **and** the
/// drain task has flushed everything into the channel.
///
/// # Panics
///
/// Panics if the shared tag-sink mutex is poisoned (a drain task panicked
/// while holding the lock).
#[tauri::command]
pub async fn browse_tags(
    state: State<'_, AppState>,
    max_tags: Option<usize>,
    channel: Channel<TagDescriptor>,
) -> DesktopResult<()> {
    let client = state.client();
    let prog_id = state.prog_id().await?;
    let max = max_tags.unwrap_or(1000);

    // `progress` is required by the `OpcProvider::browse_tags` signature
    // but we no longer poll it from the drain task — completion is
    // signalled via the `done` flag below.
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // New: explicit completion flag set by the awaiting code below.
    let done = Arc::new(AtomicBool::new(false));

    // Drain task: pump newly-arrived tags into the channel until the
    // browse call completes (Ok or Err). The previous "progress ==
    // sink.len()" heuristic could deadlock on error; we now rely on
    // the explicit `done` flag instead.
    let channel_for_drain = channel.clone();
    let sink_clone = Arc::clone(&sink);
    let done_clone = Arc::clone(&done);
    let drain = tokio::spawn(async move {
        let mut last_seen = 0usize;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // Flush newly-arrived tags (regardless of `done` state).
            let new: Vec<String> = {
                let mut g = sink_clone.lock().expect("sink poisoned");
                if g.len() > last_seen {
                    g.drain(last_seen..).collect()
                } else {
                    Vec::new()
                }
            };
            last_seen += new.len();
            for item_id in new {
                if channel_for_drain.send(TagDescriptor { item_id }).is_err() {
                    return; // WebView dropped
                }
            }
            // After flushing, if the browse is done, exit. Drain one
            // more pass is unnecessary because the browse code sets
            // `done` only after its own push loop has terminated.
            if done_clone.load(Ordering::Acquire) {
                return;
            }
        }
    });

    let result = client
        .browse_tags(
            &prog_id,
            max,
            Arc::clone(&progress),
            Arc::clone(&sink),
            0,
            0,
        )
        .await
        .map_err(DesktopError::from);

    // Signal the drain task to exit (it may still flush one last
    // batch on its next tick). Set BEFORE awaiting so the drain task
    // sees `done == true` on its very next check.
    done.store(true, Ordering::Release);
    let _ = drain.await;

    // Best-effort final flush of any tags pushed between the drain
    // task's last flush and our `done.store`.
    {
        let g = sink.lock().expect("sink poisoned");
        for item_id in g.iter() {
            let _ = channel.send(TagDescriptor {
                item_id: item_id.clone(),
            });
        }
    }

    result.map(|_| ())
}
