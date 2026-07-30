//! IPC payload types and the subscription runner that drives
//! `Channel<TagUpdate>` from a live `SubscriptionHandle::rx`.

pub mod subscription_runner;

use serde::{Deserialize, Serialize};

use opc_da_client::TagValue;

/// One update pushed to the WebView's subscription channel.
///
/// Field names mirror `opc_da_client::TagValue` (0.3.0):
/// `tag_id` / `value` / `quality` / `timestamp`. All are display strings;
/// the frontend renders them verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagUpdate {
    /// Tag identifier (fully-qualified path).
    pub tag_id: String,
    /// Display value (string form of the underlying OPC value).
    pub value: String,
    /// Server timestamp (local-time string).
    pub timestamp: String,
    /// Quality string (e.g. `"Good"` / `"Bad"` / `"Uncertain"`).
    pub quality: String,
}

impl From<TagValue> for TagUpdate {
    fn from(t: TagValue) -> Self {
        Self {
            tag_id: t.tag_id,
            value: t.value,
            timestamp: t.timestamp,
            quality: t.quality,
        }
    }
}
