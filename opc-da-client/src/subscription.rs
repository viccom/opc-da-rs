//! OPC DA subscription support.
//!
//! Provides the `IOPCDataCallback` COM sink that receives server-pushed data-change
//! notifications and forwards them as [`TagValue`]s onto a Tokio mpsc channel, bridging
//! the COM callback model into Rust async consumers.

// `#[implement]` expands generated COM glue (`_Impl`/`_Vtbl`) that trips several pedantic
// lints (raw-pointer casts, `inline(always)`, undocumented unsafe, &mut-self). They mirror the
// `#![allow]` block guarding `iterator.rs` inside `opc_da`.
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::needless_pass_by_ref_mut
)]

use crate::bindings::comn::{IOPCShutdown, IOPCShutdown_Impl};
use crate::bindings::da::{IOPCDataCallback, IOPCDataCallback_Impl};
use crate::helpers::{filetime_to_string, quality_to_string, variant_to_string};
use crate::provider::TagValue;
use std::sync::Mutex;
use tokio::sync::mpsc;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Variant::VARIANT;
use windows::core::PCWSTR;
use windows::core::implement;

/// COM callback sink receiving OPC DA data-change notifications.
///
/// Implements `IOPCDataCallback`. On each `OnDataChange`/`OnReadComplete`, the parallel
/// client-handle / value / quality / timestamp / error arrays are parsed into [`TagValue`]s
/// and forwarded via [`mpsc::Sender::try_send`] (non-blocking, so a slow consumer cannot
/// stall the COM worker thread).
#[allow(clippy::ref_as_ptr, clippy::inline_always)]
#[implement(IOPCDataCallback)]
pub struct DataCallbackSink {
    /// Tag IDs indexed by client handle (`hClient` assigned as the item index at `add_items`).
    pub tag_ids: Vec<String>,
    /// Channel forwarder guarded by a mutex (COM callbacks are external to the Rust borrow graph).
    pub tx: Mutex<mpsc::Sender<TagValue>>,
}

impl IOPCDataCallback_Impl for DataCallbackSink_Impl {
    fn OnDataChange(
        &self,
        _dwtransid: u32,
        _hgroup: u32,
        _hrmasterquality: windows::core::HRESULT,
        _hrmastererror: windows::core::HRESULT,
        dwcount: u32,
        phclientitems: *const u32,
        pvvalues: *const VARIANT,
        pwqualities: *const u16,
        pfttimestamps: *const FILETIME,
        perrors: *const windows::core::HRESULT,
    ) -> windows::core::Result<()> {
        forward_data_change(
            &self.tag_ids,
            &self.tx,
            dwcount,
            phclientitems,
            pvvalues,
            pwqualities,
            pfttimestamps,
            perrors,
        );
        Ok(())
    }

    fn OnReadComplete(
        &self,
        _dwtransid: u32,
        _hgroup: u32,
        _hrmasterquality: windows::core::HRESULT,
        _hrmastererror: windows::core::HRESULT,
        dwcount: u32,
        phclientitems: *const u32,
        pvvalues: *const VARIANT,
        pwqualities: *const u16,
        pfttimestamps: *const FILETIME,
        perrors: *const windows::core::HRESULT,
    ) -> windows::core::Result<()> {
        forward_data_change(
            &self.tag_ids,
            &self.tx,
            dwcount,
            phclientitems,
            pvvalues,
            pwqualities,
            pfttimestamps,
            perrors,
        );
        Ok(())
    }

    fn OnWriteComplete(
        &self,
        _dwtransid: u32,
        _hgroup: u32,
        _hrmastererr: windows::core::HRESULT,
        _dwcount: u32,
        _pclienthandles: *const u32,
        _perrors: *const windows::core::HRESULT,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnCancelComplete(&self, _dwtransid: u32, _hgroup: u32) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Parse a parallel OPC DA data-change array batch into `TagValue`s and forward them.
#[allow(clippy::too_many_arguments, clippy::cast_possible_truncation)]
fn forward_data_change(
    tag_ids: &[String],
    tx: &Mutex<mpsc::Sender<TagValue>>,
    dwcount: u32,
    phclientitems: *const u32,
    pvvalues: *const VARIANT,
    pwqualities: *const u16,
    pfttimestamps: *const FILETIME,
    perrors: *const windows::core::HRESULT,
) {
    if dwcount == 0
        || phclientitems.is_null()
        || pvvalues.is_null()
        || pwqualities.is_null()
        || pfttimestamps.is_null()
        || perrors.is_null()
    {
        return;
    }
    let Ok(tx) = tx.lock() else {
        return;
    };
    for i in 0..dwcount as usize {
        // SAFETY: OPC guarantees `dwcount` valid elements in each parallel array passed
        // to the callback; `i < dwcount`, so each `.add(i)` dereference is in bounds.
        let client_handle = unsafe { *phclientitems.add(i) } as usize;
        if !unsafe { *perrors.add(i) }.is_ok() {
            continue;
        }
        let Some(tag_id) = tag_ids.get(client_handle) else {
            continue;
        };
        let value = unsafe { &*pvvalues.add(i) };
        let quality = unsafe { *pwqualities.add(i) };
        let timestamp = unsafe { *pfttimestamps.add(i) };
        let tv = TagValue {
            tag_id: tag_id.clone(),
            value: variant_to_string(value),
            quality: quality_to_string(quality),
            timestamp: filetime_to_string(timestamp),
        };
        // Non-blocking: a stalled consumer must not freeze the COM worker thread.
        let _ = tx.try_send(tv);
    }
}

/// COM callback sink receiving server shutdown requests (`IOPCShutdown`).
///
/// When the server calls `ShutdownRequest`, the reason string is forwarded to the channel
/// so the consumer can trigger reconnection logic.
#[allow(clippy::ref_as_ptr, clippy::inline_always)]
#[implement(IOPCShutdown)]
pub struct ShutdownSink {
    /// Channel forwarder for the shutdown reason string.
    pub tx: Mutex<mpsc::Sender<String>>,
}

impl IOPCShutdown_Impl for ShutdownSink_Impl {
    fn ShutdownRequest(&self, szreason: &PCWSTR) -> windows::core::Result<()> {
        // SAFETY: OPC passes a valid null-terminated wide string for the reason.
        let reason = unsafe { szreason.to_string() }.unwrap_or_default();
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.try_send(reason);
        }
        Ok(())
    }
}
