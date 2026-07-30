//! `#[tauri::command]` IPC handlers, grouped by domain.
//!
//! Each submodule owns one slice of the user-facing feature set:
//!
//! - [`servers`]  — list / connect (your functions 1, 2, 3)
//! - [`browse`]   — tag tree browser for the "Add tags" modal (function 5)
//! - [`read`]     — synchronous read of current values
//! - [`write`]    — single / batch / VQT writes
//! - [`subscription`] — create / remove subscription groups (function 4) and
//!   wire a Tauri `Channel<TagUpdate>` to the live stream (function 5 real-time
//!   table)

pub mod browse;
pub mod read;
pub mod servers;
pub mod subscription;
pub mod write;

pub use browse::*;
pub use read::*;
pub use servers::*;
pub use subscription::*;
pub use write::*;