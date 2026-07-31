//! Write IPC handlers.

use serde::{Deserialize, Serialize};
use tauri::State;

use opc_da_client::OpcProvider;
use opc_da_client::OpcValue;

use crate::error::DesktopResult;
use crate::state::AppState;

/// Single-tag write payload from the WebView.
///
/// `value` is mapped to `OpcValue` by JSON shape:
///
/// - `null`        → no-op (rejected — `OpcValue` has no `Empty` variant in 0.3.0)
/// - `bool`        → `OpcValue::Bool`
/// - integer       → `OpcValue::Int(i32)`
/// - float         → `OpcValue::Float(f64)`
/// - string        → `OpcValue::String`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteRequest {
    /// Fully-qualified tag path.
    pub item_id: String,
    /// New value as JSON.
    pub value: serde_json::Value,
}

impl WriteRequest {
    /// Map a JSON value to `OpcValue`. Floats that happen to be integers stay
    /// as `Int` to avoid surprising the server. Arrays and objects are
    /// rejected — `opc-da-client` 0.3.0 does not expose a SafeArray write
    /// path.
    #[allow(clippy::cast_precision_loss)] // 超 i32 的整数回退为 Float；i64→f64 精度损失可接受（OPC Float）
    fn to_opc_value(&self) -> Option<OpcValue> {
        match &self.value {
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => None,
            serde_json::Value::Bool(b) => Some(OpcValue::Bool(*b)),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    // 超 i32 范围的整数回退为 Float，避免截断。
                    Some(match i32::try_from(i) {
                        Ok(v) => OpcValue::Int(v),
                        Err(_) => OpcValue::Float(i as f64),
                    })
                } else {
                    Some(OpcValue::Float(n.as_f64()?))
                }
            }
            serde_json::Value::String(s) => Some(OpcValue::String(s.clone())),
        }
    }
}

/// Write a single tag.
#[tauri::command]
pub async fn write_tag_value(
    state: State<'_, AppState>,
    request: WriteRequest,
) -> DesktopResult<WriteResultDto> {
    let client = state.client().await;
    let prog_id = state.prog_id().await?;
    let value = request.to_opc_value().ok_or_else(|| {
        crate::error::DesktopError::Other(
            "cannot write null value (no OpcValue::Empty in 0.3.0)".into(),
        )
    })?;
    let result = client
        .write_tag_value(&prog_id, &request.item_id, value)
        .await
        .map_err(crate::error::DesktopError::from)?;
    Ok(WriteResultDto::from(result))
}

/// Wire-friendly view of `opc_da_client::WriteResult` (which doesn't
/// derive `Serialize`). Mirrors the public fields 1:1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResultDto {
    /// The tag that was written to.
    pub tag_id: String,
    /// Whether the write succeeded.
    pub success: bool,
    /// Error message if the write failed, `None` on success.
    pub error: Option<String>,
}

impl From<opc_da_client::WriteResult> for WriteResultDto {
    fn from(r: opc_da_client::WriteResult) -> Self {
        Self {
            tag_id: r.tag_id,
            success: r.success,
            error: r.error,
        }
    }
}
