use thiserror::Error;
use windows::core::HRESULT;

/// Result type alias for OPC DA operations.
pub type OpcResult<T> = Result<T, OpcError>;

/// Centralized error enum for the OPC DA client.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpcError {
    /// Standard Windows COM/DCOM error.
    ///
    /// This variant wraps a [`windows::core::Error`] and provides a friendly
    /// hint for common OPC-related HRESULT codes.
    #[error("COM error: {source} ({})", friendly_hresult_hint(.source.code()).unwrap_or("No hint available"))]
    Com {
        #[from]
        source: windows::core::Error,
    },

    /// Connection-related errors (e.g., host unreachable, resolution failure).
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Server-specific errors reported via OPC status codes.
    #[error("Server error: {0} (0x{1:08X})")]
    Server(String, u32),

    /// Errors during data type conversion or VARIANT processing.
    #[error("Data conversion failed: {0}")]
    Conversion(String),

    /// Operation attempted in an invalid state (e.g., group already exists).
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Feature not implemented or supported by the target OPC server.
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Catch-all for unexpected internal failures.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for OpcError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<tokio::task::JoinError> for OpcError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Internal(format!("Async task join failed: {err}"))
    }
}

impl From<std::num::TryFromIntError> for OpcError {
    fn from(err: std::num::TryFromIntError) -> Self {
        OpcError::Conversion(format!("Integer conversion error: {err}"))
    }
}

/// Helper to format HRESULT with friendly hints.
pub fn format_hresult(hr: HRESULT) -> String {
    let hex = format!("0x{:08X}", hr.0 as u32);
    match friendly_hresult_hint(hr) {
        Some(hint) => format!("{hex}: {hint}"),
        None => hex,
    }
}

/// Maps known COM/DCOM error codes to actionable user hints.
pub fn friendly_hresult_hint(hr: HRESULT) -> Option<&'static str> {
    match hr.0 as u32 {
        0x80040112 => Some("Server license does not permit OPC client connections"),
        0x80080005 => Some("Server process failed to start — check if it is installed and running"),
        0x80070005 => {
            Some("Access denied — DCOM launch/activation permissions not configured for this user")
        }
        0x800706BA => {
            Some("RPC server unavailable — the target host may be offline or blocking RPC")
        }
        0x800706F4 => Some("COM marshalling error — try restarting the OPC server"),
        0x80040154 => Some("Server is not registered on this machine"),
        0x80004003 => Some("Invalid pointer (E_POINTER)"),
        0xC0040004 => Some("Server rejected write — the item may be read-only (OPC_E_BADRIGHTS)"),
        0xC0040006 => {
            Some("Data type mismatch — server cannot convert the written value (OPC_E_BADTYPE)")
        }
        0xC0040007 => Some("Item ID not found in server address space (OPC_E_UNKNOWNITEMID)"),
        0xC0040008 => Some("Item ID syntax is invalid for this server (OPC_E_INVALIDITEMID)"),
        _ => None,
    }
}

/// Maps an [`OpcError`] to a friendly COM hint if it is a COM error.
pub fn friendly_com_hint(error: &OpcError) -> Option<&'static str> {
    match error {
        OpcError::Com { source: e } => friendly_hresult_hint(e.code()),
        _ => None,
    }
}

impl OpcError {
    /// Returns the raw `HRESULT` code carried by this error, if any.
    ///
    /// Enables programmatic matching on the numeric HRESULT (e.g.
    /// `0x800706BA`, `0xC0040007`) without coupling the caller to the
    /// `windows` crate. `Com` yields `source.code()`; `Server(_, hr)` yields
    /// the reported code; all other variants return `None`.
    #[must_use]
    pub fn hr_code(&self) -> Option<i32> {
        match self {
            Self::Com { source } => Some(source.code().0),
            Self::Server(_, hr) => Some(*hr as i32),
            _ => None,
        }
    }
}
