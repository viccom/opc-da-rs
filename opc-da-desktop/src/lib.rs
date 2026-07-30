//! `opc-da-desktop` — Tauri 2.0 desktop GUI for OPC DA.
//!
//! This crate wraps [`opc_da_client`] in a Tauri application that exposes
//! async OPC operations as `#[tauri::command]` IPC endpoints and pushes
//! high-frequency subscription updates through Tauri 2 `Channel<T>`.
//!
//! # Runtime requirements
//!
//! The default `opc-da-backend` feature requires Windows (uses COM/DCOM).
//! The `test-support` feature enables `MockOpcProvider` for non-Windows CI.
//!
//! # Layout
//!
//! - [`state`] — shared application state (`Arc<OpcDaClient>`, subscription table).
//! - [`commands`] — `#[tauri::command]` IPC handlers (one module per domain).
//! - [`ipc`] — payload types and subscription runner (drives `Channel<T>`).
//!
//! See `architecture.md` and `ARCHITECTURE_DIAGRAM.md` at the workspace root
//! for the broader design context.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod commands;
pub mod error;
pub mod ipc;
pub mod state;

#[cfg(windows)]
pub mod app;

pub use error::DesktopError;
pub use state::AppState;
