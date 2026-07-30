//! Read IPC handlers.

use serde::{Deserialize, Serialize};
use tauri::State;

use opc_da_client::OpcProvider;
use opc_da_client::TagValue;

use crate::error::{DesktopError, DesktopResult};
use crate::state::AppState;

/// Read the current value of one or more tags synchronously.
///
/// `tag_ids` is the list of fully-qualified paths. Returns a row per
/// requested ID. `opc-da-client` 0.3.0's `TagValue` is intentionally
/// minimal — `tag_id` / `value` / `quality` / `timestamp` are all
/// display strings.
#[tauri::command]
pub async fn read_tag_values(
    state: State<'_, AppState>,
    tag_ids: Vec<String>,
) -> DesktopResult<Vec<TagRow>> {
    let client = state.client();
    let prog_id = state.prog_id().await?;
    let values = client
        .read_tag_values(&prog_id, tag_ids)
        .await
        .map_err(DesktopError::from)?;
    Ok(values.into_iter().map(TagRow::from).collect())
}

/// UI-side row model (mirrors the columns of the realtime table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRow {
    /// Tag identifier.
    pub tag_id: String,
    /// Display value.
    pub value: String,
    /// Server-side timestamp (already a local-time string from the OPC stack).
    pub timestamp: String,
    /// Quality string (e.g. `"Good"` / `"Bad"` / `"Uncertain"`).
    pub quality: String,
}

impl From<TagValue> for TagRow {
    fn from(t: TagValue) -> Self {
        Self {
            tag_id: t.tag_id,
            value: t.value,
            timestamp: t.timestamp,
            quality: t.quality,
        }
    }
}
