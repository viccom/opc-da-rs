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
//! **Windows only** — OPC DA is built on COM/DCOM.

mod com_guard;
pub(crate) use com_guard::ComGuard;
mod helpers;
mod provider;

#[cfg(feature = "opc-da-backend")]
#[allow(warnings)]
mod bindings;
pub mod com_worker;

#[cfg(feature = "opc-da-backend")]
#[allow(warnings)]
mod opc_da;

#[cfg(feature = "opc-da-backend")]
mod backend;

#[cfg(feature = "opc-da-backend")]
mod subscription;

// Stable public API
pub use helpers::{format_hresult, friendly_com_hint};
pub use provider::{
    BranchNode, BrowseChildren, ItemProperty, LeafNode, OpcProvider, OpcValue, ShutdownHandle,
    SubscriptionHandle, TagValue, WriteResult,
};

#[cfg(feature = "opc-da-backend")]
pub use opc_da::{
    errors::{OpcError, OpcResult},
    typedefs::{GroupHandle, ItemHandle, ServerState, ServerStatus},
};

// Backend re-exports (conditional)
#[cfg(feature = "opc-da-backend")]
pub use backend::{connector::ComConnector, opc_da::OpcDaClient};

// Test support re-export
#[cfg(feature = "test-support")]
pub use provider::MockOpcProvider;
