//! Error type for the desktop IPC layer.
//!
//! Wraps [`opc_da_client::OpcError`] with a serde-friendly shape so Tauri
//! can return it directly across the IPC boundary.

use serde::{Deserialize, Serialize};

/// Errors surfaced from `#[tauri::command]` handlers back to the WebView.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    /// The OPC backend returned an error.
    #[error("OPC error: {0}")]
    Opc(#[from] opc_da_client::OpcError),

    /// Tauri runtime / state error (missing state, channel closed, etc.).
    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),

    /// JSON serialization failure (should be unreachable in practice).
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Caller asked for an unknown server handle or subscription.
    #[error("not found: {0}")]
    NotFound(String),

    /// Operation requested without a connected client.
    #[error("not connected: call `connect` first")]
    NotConnected,

    /// Other errors (catch-all).
    #[error("{0}")]
    Other(String),
}

impl DesktopError {
    /// Build a `DesktopError::Other` from any `Display`-able value.
    pub fn other(msg: impl std::fmt::Display) -> Self {
        Self::Other(msg.to_string())
    }
}

/// Serialize `DesktopError` as a structured object so the JS side can render it.
///
/// Shape: `{ "kind": "opc" | "tauri" | ... , "message": "..." }`.
impl Serialize for DesktopError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        #[derive(Serialize)]
        struct Repr<'a> {
            kind: &'a str,
            message: String,
        }
        let kind = match self {
            Self::Opc(_) => "opc",
            Self::Tauri(_) => "tauri",
            Self::Serde(_) => "serde",
            Self::NotFound(_) => "not_found",
            Self::NotConnected => "not_connected",
            Self::Other(_) => "other",
        };
        Repr { kind, message: self.to_string() }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DesktopError {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        // Inbound errors from JS are not expected; we accept any and turn into
        // `Other` so deserialization cannot fail the IPC pipeline.
        Ok(Self::Other("(deserialized from JS)".into()))
    }
}

/// Convenience result alias used by every `#[tauri::command]`.
pub type DesktopResult<T> = Result<T, DesktopError>;

// `tauri::ipc::InvokeError` has a blanket `impl<T: Serialize> From<T> for InvokeError`,
// so our manual `Serialize` impl on `DesktopError` is enough — no custom
// `From` is needed (and one would conflict with that blanket).