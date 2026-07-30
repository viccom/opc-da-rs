#[cfg(feature = "opc-da-backend")]
use crate::opc_da::client::ClientTrait;
use crate::opc_da::errors::{OpcError, OpcResult};
use crate::provider::OpcValue;
use windows::Win32::Foundation::{FILETIME, VARIANT_BOOL};
use windows::Win32::System::Com::{CLSIDFromProgID, CoTaskMemFree, ProgIDFromCLSID};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayGetDim, SafeArrayGetElemsize, SafeArrayGetLBound,
    SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_BSTR, VT_I4, VT_R8};
use windows::core::{BSTR, PCWSTR};

pub use crate::opc_da::errors::{
    format_hresult, friendly_com_hint, friendly_hresult_hint as friendly_com_hresult_hint,
};

// Verify GUID memory layout assumption for FFI (Workstream C#3)
const _: () = assert!(
    std::mem::size_of::<windows::core::GUID>() == 16,
    "windows::core::GUID must be 16 bytes for COM compatibility"
);
const _: () = assert!(
    std::mem::align_of::<windows::core::GUID>() >= 4,
    "windows::core::GUID must be at least 4-byte aligned"
);

/// Helper to convert GUID to `ProgID` using Windows API
pub fn guid_to_progid(guid: &windows::core::GUID) -> OpcResult<String> {
    // SAFETY: `ProgIDFromCLSID` is a Win32 FFI call that allocates a PWSTR
    // via the COM allocator. We read it and free it with `CoTaskMemFree`
    // before returning — the pointer is not used after free.
    unsafe {
        let progid = ProgIDFromCLSID(guid)
            .map_err(|e| OpcError::Internal(format!("Failed to get ProgID from CLSID: {e}")))?;

        let result = if progid.is_null() {
            String::new()
        } else {
            progid
                .to_string()
                .map_err(|e| OpcError::Conversion(format!("Failed into convert PWSTR: {e}")))?
        };

        if !progid.is_null() {
            CoTaskMemFree(Some(progid.as_ptr() as *const _));
        }

        Ok(result)
    }
}

/// Convert OPC DA VARIANT to a displayable string.
#[allow(clippy::too_many_lines)]
pub fn variant_to_string(variant: &VARIANT) -> String {
    // SAFETY: Accessing the VARIANT union fields. The caller (OpcDaClient)
    // guarantees the VARIANT was produced by COM (e.g., from `group.read()`),
    // so the `vt` discriminant correctly identifies which union arm is active.
    unsafe {
        let vt = variant.Anonymous.Anonymous.vt;
        let base_type = vt.0 & 0x0FFF; // strip VT_ARRAY (0x2000) / VT_BYREF (0x4000)
        let is_array = (vt.0 & 0x2000) != 0;

        if is_array {
            // Iterate 1-D SafeArrays and display actual element values
            let parray = variant.Anonymous.Anonymous.Anonymous.parray;
            if parray.is_null() {
                return "Array[?]".to_string();
            }
            let dims = SafeArrayGetDim(parray);
            if dims == 0 {
                return "Array[0]".to_string();
            }
            // For 1-D arrays compute count; for multi-dim just show dims
            if dims == 1 {
                let lb = SafeArrayGetLBound(parray, 1).unwrap_or(0);
                let ub = SafeArrayGetUBound(parray, 1).unwrap_or(-1);
                let count = (ub - lb + 1).max(0);
                let mut elements = Vec::new();
                let display_count = count.min(20);

                if base_type == windows::Win32::System::Variant::VT_VARIANT.0 {
                    let mut data_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                    if SafeArrayAccessData(parray, &raw mut data_ptr).is_ok() {
                        #[allow(clippy::cast_sign_loss)]
                        let vars =
                            std::slice::from_raw_parts(data_ptr as *const VARIANT, count as usize);
                        for i in 0..display_count {
                            #[allow(clippy::cast_sign_loss)]
                            elements.push(variant_to_string(&vars[i as usize]));
                        }
                        let _ = SafeArrayUnaccessData(parray);
                    }
                } else {
                    let elem_size = SafeArrayGetElemsize(parray) as usize;
                    let mut data_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                    if SafeArrayAccessData(parray, &raw mut data_ptr).is_ok() {
                        for i in 0..display_count {
                            let mut temp_var = VARIANT::default();
                            (*temp_var.Anonymous.Anonymous).vt =
                                windows::Win32::System::Variant::VARENUM(base_type);

                            #[allow(clippy::cast_sign_loss)]
                            let src_ptr = (data_ptr as *const u8).add((i as usize) * elem_size);
                            let dst_ptr =
                                std::ptr::addr_of_mut!((*temp_var.Anonymous.Anonymous).Anonymous)
                                    .cast::<u8>();

                            std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, elem_size.min(16));

                            elements.push(variant_to_string(&temp_var));
                        }
                        let _ = SafeArrayUnaccessData(parray);
                    }
                }

                let elided = if count > 20 { ", ..." } else { "" };
                return format!("[{}{elided}]", elements.join(", "));
            }
            return format!("Array[{dims}D]");
        }

        match vt.0 {
            0 => "Empty".to_string(), // VT_EMPTY
            1 => "Null".to_string(),  // VT_NULL
            2 => format!("{val}", val = variant.Anonymous.Anonymous.Anonymous.iVal), // VT_I2
            3 => format!("{val}", val = variant.Anonymous.Anonymous.Anonymous.lVal), // VT_I4
            4 => format!(
                "{val:.2}",
                val = variant.Anonymous.Anonymous.Anonymous.fltVal
            ), // VT_R4
            5 => format!(
                "{val:.2}",
                val = variant.Anonymous.Anonymous.Anonymous.dblVal
            ), // VT_R8
            6 => {
                // VT_CY - currency, 64-bit fixed-point scaled by 10,000
                let raw = variant.Anonymous.Anonymous.Anonymous.cyVal.int64;
                let whole = raw / 10_000;
                let frac = (raw % 10_000).unsigned_abs();
                format!("{whole}.{frac:04}")
            }
            7 => {
                // VT_DATE - OLE Automation date (f64, day 0 = 1899-12-30)
                let ole_date = variant.Anonymous.Anonymous.Anonymous.date;
                ole_date_to_string(ole_date)
            }
            8 => {
                // VT_BSTR - string
                let bstr = &variant.Anonymous.Anonymous.Anonymous.bstrVal;
                if bstr.is_empty() {
                    "\"\"".to_string()
                } else {
                    format!("\"{}\"", **bstr)
                }
            }
            10 => {
                // VT_ERROR - contains an HRESULT status code
                let scode = variant.Anonymous.Anonymous.Anonymous.scode;
                let hr = windows::core::HRESULT(scode);
                let hex = format!("0x{:08X}", hr.0.cast_unsigned());
                match friendly_com_hresult_hint(hr) {
                    Some(msg) => format!("Error: {msg} ({hex})"),
                    None => format!("Error ({hex})"),
                }
            }
            11 => format!(
                "{val}",
                val = variant.Anonymous.Anonymous.Anonymous.boolVal.0 != 0
            ), // VT_BOOL
            16 => {
                #[allow(clippy::cast_possible_wrap)]
                let val = variant.Anonymous.Anonymous.Anonymous.bVal as i8;
                format!("{val}")
            } // VT_I1
            17 => format!("{val}", val = variant.Anonymous.Anonymous.Anonymous.bVal), // VT_UI1
            18 => format!("{val}", val = variant.Anonymous.Anonymous.Anonymous.uiVal), // VT_UI2
            19 => format!("{val}", val = variant.Anonymous.Anonymous.Anonymous.ulVal), // VT_UI4
            20 => {
                // VT_I8: read 8 bytes as i64 via pointer cast
                let p = (&raw const variant.Anonymous.Anonymous.Anonymous).cast::<i64>();
                // SAFETY: p is a valid pointer to the variant union
                let val = *p;
                format!("{val}")
            }
            21 => {
                // VT_UI8: read 8 bytes as u64 via pointer cast
                let p = (&raw const variant.Anonymous.Anonymous.Anonymous).cast::<u64>();
                // SAFETY: p is a valid pointer to the variant union
                let val = *p;
                format!("{val}")
            }
            _ => format!("(VT {vt:?})"),
        }
    }
}

/// Convert an OLE Automation date (f64) to a local datetime string.
/// OLE date epoch is 1899-12-30; integer part = days, fraction = time-of-day.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
fn ole_date_to_string(ole_date: f64) -> String {
    // OLE epoch: 1899-12-30 00:00:00
    const OLE_EPOCH_DAYS: i64 = 25569; // days from 1899-12-30 to 1970-01-01
    let total_secs = (ole_date - OLE_EPOCH_DAYS as f64) * 86400.0;
    chrono::DateTime::from_timestamp(total_secs as i64, 0).map_or_else(
        || format!("{ole_date:.6}"),
        |utc| {
            utc.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}

/// Map OPC quality code to a human-readable label.
pub fn quality_to_string(quality: u16) -> String {
    let quality_bits = quality & 0xC0; // Top 2 bits define Good/Bad/Uncertain
    match quality_bits {
        0xC0 => "Good".to_string(),
        0x00 => "Bad".to_string(),
        0x40 => "Uncertain".to_string(),
        _ => format!("Unknown(0x{quality:04X})"),
    }
}

/// Convert FILETIME to a human-readable local time string.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
pub fn filetime_to_string(ft: FILETIME) -> String {
    if ft.dwHighDateTime == 0 && ft.dwLowDateTime == 0 {
        return "N/A".to_string();
    }
    let intervals = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    let unix_secs = (intervals / 10_000_000).saturating_sub(11_644_473_600);
    let nanos = ((intervals % 10_000_000) * 100) as u32;

    chrono::DateTime::from_timestamp(unix_secs as i64, nanos).map_or_else(
        || "Invalid".to_string(),
        |utc| {
            utc.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}

/// Convert an [`OpcValue`] into a COM [`VARIANT`] for writing.
pub fn opc_value_to_variant(value: &OpcValue) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: We set the `vt` discriminant and the corresponding union
    // field atomically. The VARIANT is returned by value, so no aliasing.
    // `ManuallyDrop` on BSTR prevents double-free — COM takes ownership.
    unsafe {
        match value {
            OpcValue::String(s) => {
                (*variant.Anonymous.Anonymous).vt = VT_BSTR;
                (*variant.Anonymous.Anonymous).Anonymous.bstrVal =
                    std::mem::ManuallyDrop::new(BSTR::from(s));
            }
            OpcValue::Int(i) => {
                (*variant.Anonymous.Anonymous).vt = VT_I4;
                (*variant.Anonymous.Anonymous).Anonymous.lVal = *i;
            }
            OpcValue::Float(f) => {
                (*variant.Anonymous.Anonymous).vt = VT_R8;
                (*variant.Anonymous.Anonymous).Anonymous.dblVal = *f;
            }
            OpcValue::Bool(b) => {
                (*variant.Anonymous.Anonymous).vt = VT_BOOL;
                (*variant.Anonymous.Anonymous).Anonymous.boolVal =
                    VARIANT_BOOL(if *b { -1 } else { 0 });
            }
        }
    }
    variant
}

/// Whether `host` selects the DCOM path (`CoCreateInstanceEx` + `COSERVERINFO`) vs local
/// `CoCreateInstance`.
///
/// Empty host = local; any non-empty host (including `"localhost"`) = DCOM. `"localhost"` is
/// deliberately treated as DCOM: a local `CoCreateInstance` with `CLSCTX_ALL` may attempt an
/// in-proc DLL and fail to launch the out-of-proc OPC server, whereas the DCOM roundtrip to the
/// loopback reliably starts it — matching the behaviour of `127.0.0.1`.
pub fn is_remote_host(host: &str) -> bool {
    !host.is_empty()
}

/// Resolve an OPC DA server `ProgID` to a connected `opc_da` Server instance.
///
/// Converts the `ProgID` string to a `CLSID` via the Windows registry,
/// then creates and returns a connected server handle.
///
/// # Errors
///
/// Returns `Err` if the `ProgID` cannot be resolved or the server
/// cannot be instantiated.
pub fn connect_server(server_name: &str, host: &str) -> OpcResult<crate::bindings::da::IOPCServer> {
    // SAFETY: `server_wide` is null-terminated and lives until the end of this scope.
    let clsid_raw = unsafe {
        let server_wide: Vec<u16> = server_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        CLSIDFromProgID(PCWSTR(server_wide.as_ptr())).map_err(|e| {
            OpcError::Connection(format!(
                "Failed to resolve ProgID '{server_name}' to CLSID: {e}"
            ))
        })?
    };
    // SAFETY: `opc_da::GUID` and `windows::core::GUID` are binary compatible 128-bit structs.
    let clsid = unsafe { std::mem::transmute_copy(&clsid_raw) };

    let client = crate::opc_da::client::v2::Client;
    let is_remote = is_remote_host(host);
    let server_result = if is_remote {
        let info = crate::opc_da::typedefs::ServerInfo {
            name: host.to_string(),
            auth_info: crate::opc_da::typedefs::AuthInfo::default_dcom(),
        };
        tracing::debug!(server = %server_name, host = %host, "Connecting via remote DCOM (create_server2)");
        client.create_server2(
            clsid,
            crate::opc_da::typedefs::ClassContext::All,
            Some(info),
        )
    } else {
        client.create_server(clsid, crate::opc_da::typedefs::ClassContext::All)
    };
    let server = server_result.map_err(|e| {
        let hint = if let OpcError::Com { ref source } = e {
            friendly_com_hresult_hint(source.code())
        } else {
            None
        }
        .unwrap_or("Check DCOM configuration and server status");
        tracing::error!(error = ?e, server = %server_name, host = %host, hint, "create_server failed");
        e
    })?;
    tracing::debug!(server = %server_name, host = %host, "Connected to OPC DA server");
    Ok(server.server)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::single_char_pattern,
        clippy::cast_possible_wrap,
        clippy::ptr_as_ptr,
        clippy::borrow_as_ptr,
        clippy::mixed_attributes_style,
        clippy::unreadable_literal
    )]
    use super::*;

    #[test]
    fn test_is_remote_host() {
        // localhost 走 DCOM（同 127.0.0.1，绕过本地 CoCreateInstance 的 in-proc 尝试）；
        // 空 host 走本地 CoCreateInstance。
        assert!(!is_remote_host(""));
        assert!(is_remote_host("localhost"));
        assert!(is_remote_host("LOCALHOST"));
        assert!(is_remote_host("127.0.0.1"));
        assert!(is_remote_host("192.168.1.10"));
    }

    #[test]
    fn test_friendly_com_hint_known_codes() {
        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0x800706F4_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("COM marshalling error — try restarting the OPC server")
        );

        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80040154_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("Server is not registered on this machine")
        );

        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0xC0040004_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("Server rejected write — the item may be read-only (OPC_E_BADRIGHTS)"),
        );

        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0xC0040006_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("Data type mismatch — server cannot convert the written value (OPC_E_BADTYPE)"),
        );

        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0xC0040007_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("Item ID not found in server address space (OPC_E_UNKNOWNITEMID)"),
        );

        let err = OpcError::Com {
            source: windows::core::Error::from_hresult(windows::core::HRESULT(
                0xC0040008_u32 as i32,
            )),
        };
        assert_eq!(
            friendly_com_hint(&err),
            Some("Item ID syntax is invalid for this server (OPC_E_INVALIDITEMID)"),
        );
    }

    #[test]
    fn test_friendly_com_hint_unknown_code() {
        let err = OpcError::Internal("Some other error".to_string());
        assert_eq!(friendly_com_hint(&err), None);
    }

    #[test]
    fn test_filetime_to_string_zero() {
        let ft = FILETIME {
            dwHighDateTime: 0,
            dwLowDateTime: 0,
        };
        assert_eq!(filetime_to_string(ft), "N/A");
    }

    #[test]
    fn test_filetime_to_string_nonzero() {
        let ft = FILETIME {
            dwHighDateTime: 0x01DC_9EF1,
            dwLowDateTime: 0x0A3B_DF80,
        };
        let result = filetime_to_string(ft);
        assert!(result.contains("-"));
    }
    #[test]
    fn test_opc_value_to_variant_int() {
        let v = opc_value_to_variant(&OpcValue::Int(42));
        // SAFETY: The bytes vector length matches the chunk size and element boundaries.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_I4);
            assert_eq!(v.Anonymous.Anonymous.Anonymous.lVal, 42);
        }
    }

    #[test]
    fn test_opc_value_to_variant_float() {
        let v = opc_value_to_variant(&OpcValue::Float(3.5));
        // SAFETY: The bytes vector contains standard memory representation for elements.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_R8);
            assert!((v.Anonymous.Anonymous.Anonymous.dblVal - 3.5).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_opc_value_to_variant_bool_true() {
        let v = opc_value_to_variant(&OpcValue::Bool(true));
        // SAFETY: Same as above.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_BOOL);
            assert_eq!(v.Anonymous.Anonymous.Anonymous.boolVal.0, -1);
        }
    }

    #[test]
    fn test_opc_value_to_variant_bool_false() {
        let v = opc_value_to_variant(&OpcValue::Bool(false));
        // SAFETY: Creating variant union from safe array requires unsafe blocks
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_BOOL);
            assert_eq!(v.Anonymous.Anonymous.Anonymous.boolVal.0, 0);
        }
    }

    #[test]
    fn test_opc_value_to_variant_string() {
        let v = opc_value_to_variant(&OpcValue::String("hello".into()));
        // SAFETY: Same as above.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_BSTR);
            let bstr = &v.Anonymous.Anonymous.Anonymous.bstrVal;
            assert_eq!(&**bstr, "hello");
        }
    }

    #[test]
    fn test_variant_roundtrip() {
        // Int roundtrip
        let v = opc_value_to_variant(&OpcValue::Int(99));
        assert_eq!(variant_to_string(&v), "99");

        // Float roundtrip
        let v = opc_value_to_variant(&OpcValue::Float(3.5));
        assert_eq!(variant_to_string(&v), "3.50");

        // Bool true roundtrip
        let v = opc_value_to_variant(&OpcValue::Bool(true));
        assert_eq!(variant_to_string(&v), "true");

        // Bool false roundtrip
        let v = opc_value_to_variant(&OpcValue::Bool(false));
        assert_eq!(variant_to_string(&v), "false");

        // String roundtrip
        let v = opc_value_to_variant(&OpcValue::String("world".into()));
        assert_eq!(variant_to_string(&v), "\"world\"");
    }

    #[test]
    fn test_variant_to_string_cy() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Com::CY;
        use windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_CY,
        };

        {
            let cy_val = CY { int64: 123_456_789 };
            let inner_union = VARIANT_0_0_0 { cyVal: cy_val };
            let middle_struct = VARIANT_0_0 {
                vt: VT_CY,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: inner_union,
            };
            let outer_union = VARIANT_0 {
                Anonymous: ManuallyDrop::new(middle_struct),
            };
            let v = VARIANT {
                Anonymous: outer_union,
            };
            assert_eq!(variant_to_string(&v), "12345.6789");
        }

        {
            let cy_val = CY { int64: -500_001 };
            let inner_union = VARIANT_0_0_0 { cyVal: cy_val };
            let middle_struct = VARIANT_0_0 {
                vt: VT_CY,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: inner_union,
            };
            let outer_union = VARIANT_0 {
                Anonymous: ManuallyDrop::new(middle_struct),
            };
            let v = VARIANT {
                Anonymous: outer_union,
            };
            assert_eq!(variant_to_string(&v), "-50.0001");
        }
    }

    #[test]
    fn test_variant_to_string_empty() {
        let v = VARIANT::default();
        assert_eq!(variant_to_string(&v), "Empty");
    }

    #[test]
    fn quality_good() {
        assert_eq!(quality_to_string(0xC0), "Good");
        assert_eq!(quality_to_string(0xC4), "Good"); // sub-status bits preserved
    }

    #[test]
    fn quality_bad() {
        assert_eq!(quality_to_string(0x00), "Bad");
        assert_eq!(quality_to_string(0x04), "Bad"); // sub-status bits preserved
    }

    #[test]
    fn quality_uncertain() {
        assert_eq!(quality_to_string(0x40), "Uncertain");
    }

    #[test]
    fn quality_unknown() {
        let result = quality_to_string(0x80);
        assert!(result.starts_with("Unknown("));
    }

    #[test]
    fn test_variant_to_string_null() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_NULL,
        };

        let inner = VARIANT_0_0_0 { llVal: 0 };
        let middle = VARIANT_0_0 {
            vt: VT_NULL,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };
        assert_eq!(variant_to_string(&v), "Null");
    }

    #[test]
    fn test_variant_to_string_i2_and_r4() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I2, VT_R4,
        };

        // VT_I2
        let inner = VARIANT_0_0_0 { iVal: -42 };
        let middle = VARIANT_0_0 {
            vt: VT_I2,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };
        assert_eq!(variant_to_string(&v), "-42");

        // VT_R4
        let inner = VARIANT_0_0_0 { fltVal: 1.5 };
        let middle = VARIANT_0_0 {
            vt: VT_R4,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };
        assert_eq!(variant_to_string(&v), "1.50");
    }

    #[test]
    fn test_variant_to_string_unknown_vt() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Variant::{
            VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0,
        };

        let inner = VARIANT_0_0_0 { llVal: 0 };
        let middle = VARIANT_0_0 {
            vt: VARENUM(999),
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };
        let result = variant_to_string(&v);
        assert!(
            result.starts_with("(VT "),
            "Expected '(VT ...)' but got: {}",
            result
        );
    }
    #[test]
    fn test_variant_to_string_safearray_i4() {
        use std::ffi::c_void;
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Ole::{
            SafeArrayAccessData, SafeArrayCreateVector, SafeArrayUnaccessData,
        };
        use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VT_ARRAY, VT_I4};

        // SAFETY: Array creation and access follow standard COM patterns
        unsafe {
            let parray = SafeArrayCreateVector(VT_I4, 0, 3);
            let mut ptr: *mut c_void = std::ptr::null_mut();
            SafeArrayAccessData(parray, &mut ptr).unwrap();
            let slice = std::slice::from_raw_parts_mut(ptr as *mut i32, 3);
            slice[0] = 10;
            slice[1] = 20;
            slice[2] = 30;
            SafeArrayUnaccessData(parray).unwrap();

            let mut middle = VARIANT_0_0 {
                vt: windows::Win32::System::Variant::VARENUM(VT_I4.0 | VT_ARRAY.0),
                ..Default::default()
            };
            middle.Anonymous.parray = parray;

            let v = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(middle),
                },
            };

            assert_eq!(variant_to_string(&v), "[10, 20, 30]");
        }
    }

    #[test]
    fn test_variant_to_string_vt_error_known() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Variant::{
            VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0,
        };

        // 0xC0040007 is OPC_E_UNKNOWNITEMID
        let inner = VARIANT_0_0_0 {
            scode: -1_073_479_673,
        }; // 0xC0040007 as i32
        let middle = VARIANT_0_0 {
            vt: VARENUM(10), // VT_ERROR
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };

        assert_eq!(
            super::variant_to_string(&v),
            "Error: Item ID not found in server address space (OPC_E_UNKNOWNITEMID) (0xC0040007)"
        );
    }

    #[test]
    fn test_variant_to_string_vt_error_unknown() {
        use std::mem::ManuallyDrop;
        use windows::Win32::System::Variant::{
            VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0,
        };

        let inner = VARIANT_0_0_0 {
            scode: -559_038_737,
        }; // 0xDEADBEEF as i32
        let middle = VARIANT_0_0 {
            vt: VARENUM(10), // VT_ERROR
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: inner,
        };
        let outer = VARIANT_0 {
            Anonymous: ManuallyDrop::new(middle),
        };
        let v = VARIANT { Anonymous: outer };

        assert_eq!(super::variant_to_string(&v), "Error (0xDEADBEEF)");
    }

    #[test]
    fn test_format_hresult_known() {
        // 0x80040154 is REGDB_E_CLASSNOTREG
        let hr = windows::core::HRESULT(0x8004_0154_u32 as i32);
        assert_eq!(
            super::format_hresult(hr),
            "0x80040154: Server is not registered on this machine"
        );
    }

    #[test]
    fn test_format_hresult_unknown() {
        let hr = windows::core::HRESULT(0x1234_5678_u32 as i32);
        assert_eq!(super::format_hresult(hr), "0x12345678");
    }
}
