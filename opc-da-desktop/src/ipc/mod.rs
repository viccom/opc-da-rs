//! IPC payload types and the subscription runner that drives
//! `Channel<TagUpdate>` from a live `SubscriptionHandle::rx`.

pub mod fusion_runner;
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
    /// OPC data type display name (e.g. `"Float"`, `"Boolean"`, `"Array of String"`).
    pub data_type: String,
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
            data_type: t.data_type,
            timestamp: t.timestamp,
            quality: t.quality,
        }
    }
}

/// `FusionReader` 推给前端的事件（订阅优先 + 订阅不通自动同步兜底）。
///
/// `#[serde(tag = "kind")]` 让前端按 `ev.kind` 判别：`"Data"` / `"Subscribed"` /
/// `"Fallback"`。`Data` 复用 [`TagUpdate`]，前端订阅表格渲染逻辑一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FusionEventDto {
    /// 一条标签值（订阅推送或同步兜底读产生）。
    Data(TagUpdate),
    /// 订阅建立成功，进入推送模式。
    Subscribed,
    /// 切到同步兜底，携带原因（前端在 group 状态显示）。
    Fallback {
        /// 兜底原因（订阅失败 / 回调静默 / 推送流关闭等）。
        message: String,
    },
}
