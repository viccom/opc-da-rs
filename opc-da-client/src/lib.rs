#![allow(unsafe_code, unreachable_pub)]
#![doc = include_str!("../README.md")]
//! # opc-da-client
//!
//! Backend-agnostic OPC DA client library for Rust — async, trait-based,
//! with transparent COM management.
//!
//! ## Quick Start
//!
//! ```no_run
//! # use anyhow::Result;
//! use opc_da_client::{OpcDaClient, OpcProvider};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! let client = OpcDaClient::default();
//! let servers = client.list_servers("localhost").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! | Flag | Default | Effect |
//! |------|---------|--------|
//! | `opc-da-backend` | ✅ | Native OPC DA backend via `windows-rs` |
//! | `test-support` | ❌ | Enables `MockOpcProvider` via `mockall` |
//!
//! ## Platform
//!
//! **Windows only** — OPC DA is built on COM/DCOM. Building on any other
//! target yields a single `compile_error!` (see below).

// Non-Windows targets get one friendly error instead of a cascade of
// unresolved-import errors from the Windows-only COM backend.
#[cfg(not(target_os = "windows"))]
compile_error!(
    "opc-da-client requires Windows (COM/DCOM). It cannot be built on non-Windows targets."
);

// All COM/DCOM functionality is Windows-only, and the native backend lives
// behind the `opc-da-backend` feature. Together these bound every module and
// re-export below. On Windows + default features this is a no-op (all true);
// on Windows + `--no-default-features` the crate compiles to an empty shell;
// on non-Windows only the `compile_error!` above remains.
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod com_guard;
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub(crate) use com_guard::ComGuard;
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod helpers;
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod provider;

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
#[allow(warnings)]
mod bindings;
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub mod com_worker;

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
#[allow(warnings)]
mod opc_da;

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod backend;

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod fusion_reader;
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
mod subscription;

// Stable public API (Windows + backend only)
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub use helpers::{format_hresult, friendly_com_hint};
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub use provider::{
    BranchNode, BrowseChildren, ItemProperty, LeafNode, OpcProvider, OpcValue, ShutdownHandle,
    SubscriptionHandle, TagValue, WriteResult,
};

// Backend re-exports (conditional)
#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub use opc_da::{
    errors::{OpcError, OpcResult},
    typedefs::{AuthCredentials, GroupHandle, ItemHandle, ServerState, ServerStatus},
};

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub use backend::{connector::ComConnector, opc_da::OpcDaClient};

#[cfg(all(target_os = "windows", feature = "opc-da-backend"))]
pub use fusion_reader::{FusionEvent, FusionReader, FusionReaderOptions};

// Test support re-export (requires the backend, which is Windows-only).
#[cfg(all(
    target_os = "windows",
    feature = "opc-da-backend",
    feature = "test-support"
))]
pub use provider::MockOpcProvider;
