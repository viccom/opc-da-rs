use windows_core::Interface as _;

use crate::opc_da::{
    client::GuidIterator,
    com_utils::{IntoBridge, ToNative, TryToNative as _},
    errors::{OpcError, OpcResult},
    typedefs::{ClassContext, ServerInfo},
};

/// Trait defining client functionality for OPC Data Access servers.
pub trait ClientTrait<Server: TryFrom<windows::core::IUnknown, Error = windows::core::Error>> {
    /// GUID of the catalog used to enumerate servers.
    const CATALOG_ID: windows::core::GUID;

    /// Retrieves an iterator over available server GUIDs.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `GuidIterator` over server GUIDs, or an error if the operation fails.
    fn get_servers(&self, host: Option<&str>) -> OpcResult<GuidIterator> {
        tracing::debug!("Enumerating OPC DA Server classes via COM Component Categories Manager");
        let id = unsafe {
            windows::Win32::System::Com::CLSIDFromProgID(windows::core::w!("OPC.ServerList.1"))?
        };

        let servers: crate::bindings::comn::IOPCServerList = match host {
            Some(h) if !h.is_empty() && !h.eq_ignore_ascii_case("localhost") => {
                // SAFETY: `host_wide` is null-terminated and outlives the CoCreateInstanceEx call.
                let mut host_wide: Vec<u16> = h.encode_utf16().chain(core::iter::once(0)).collect();
                let native = windows::Win32::System::Com::COSERVERINFO {
                    dwReserved1: 0,
                    pwszName: windows::core::PWSTR(host_wide.as_ptr().cast_mut()),
                    // pAuthInfo = null => DCOM default authentication (current logged-in user),
                    // matching legacy 32-bit OPC clients. Avoids the AuthInfo bridge's dangling
                    // pointer to a temporary COAUTHINFO (caused 0x800703E6 on 32-bit).
                    pAuthInfo: std::ptr::null_mut(),
                    dwReserved2: 0,
                };
                let mut results = [windows::Win32::System::Com::MULTI_QI {
                    pIID: &crate::bindings::comn::IOPCServerList::IID,
                    pItf: core::mem::ManuallyDrop::new(None),
                    hr: windows::core::HRESULT(0),
                }];
                // SAFETY: CoCreateInstanceEx with a remote COSERVERINFO instantiates
                // IOPCServerList on `host` and returns it via MULTI_QI.
                unsafe {
                    windows::Win32::System::Com::CoCreateInstanceEx(
                        &id,
                        None,
                        windows::Win32::System::Com::CLSCTX_ALL,
                        Some(&native),
                        &mut results,
                    )?;
                }
                if results[0].hr.is_err() {
                    return Err(OpcError::Com {
                        source: results[0].hr.into(),
                    });
                }
                results[0]
                    .pItf
                    .as_ref()
                    .ok_or_else(|| {
                        OpcError::Internal("CoCreateInstanceEx returned null IOPCServerList".into())
                    })?
                    .cast()?
            }
            _ => unsafe {
                windows::Win32::System::Com::CoCreateInstance(
                    &id,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )?
            },
        };

        let versions = [Self::CATALOG_ID];

        let iter = unsafe {
            servers
                .EnumClassesOfCategories(&versions, &versions)
                .map_err(|e| {
                    windows::core::Error::new(e.code(), "Failed to enumerate server classes")
                })?
        };

        Ok(GuidIterator::new(iter))
    }

    /// Creates a server instance from the specified class ID.
    ///
    /// # Parameters
    ///
    /// - `class_id`: The GUID of the server class to instantiate.
    ///
    /// # Returns
    ///
    /// A `Result` containing the server instance, or an error if creation fails.
    fn create_server(
        &self,
        class_id: windows::core::GUID,
        class_context: ClassContext,
    ) -> OpcResult<Server> {
        tracing::debug!(
            ?class_id,
            ?class_context,
            "Creating OPC server instance via COM CoCreateInstance"
        );
        let server: crate::bindings::da::IOPCServer = unsafe {
            windows::Win32::System::Com::CoCreateInstance(
                &class_id,
                None,
                class_context.to_native(),
            )?
        };

        server
            .cast::<windows::core::IUnknown>()?
            .try_into()
            .map_err(|source| OpcError::Com { source })
    }

    fn create_server2(
        &self,
        class_id: windows::core::GUID,
        class_context: ClassContext,
        server_info: Option<ServerInfo>,
    ) -> OpcResult<Server> {
        let mut results = [windows::Win32::System::Com::MULTI_QI {
            pIID: &windows::core::IUnknown::IID,
            pItf: core::mem::ManuallyDrop::new(None),
            hr: windows::core::HRESULT(0),
        }];

        // SAFETY: when remote, `host_wide` + `native` outlive the CoCreateInstanceEx call.
        unsafe {
            match &server_info {
                Some(info) => {
                    let mut host_wide: Vec<u16> = info
                        .name
                        .encode_utf16()
                        .chain(core::iter::once(0))
                        .collect();
                    let native = windows::Win32::System::Com::COSERVERINFO {
                        dwReserved1: 0,
                        pwszName: windows::core::PWSTR(host_wide.as_ptr().cast_mut()),
                        // pAuthInfo = null => DCOM default auth (current logged-in user),
                        // matching legacy 32-bit clients; avoids the AuthInfo bridge's
                        // dangling pointer to a temporary COAUTHINFO.
                        pAuthInfo: std::ptr::null_mut(),
                        dwReserved2: 0,
                    };
                    windows::Win32::System::Com::CoCreateInstanceEx(
                        &class_id,
                        None,
                        class_context.to_native(),
                        Some(&native),
                        &mut results,
                    )?
                }
                None => windows::Win32::System::Com::CoCreateInstanceEx(
                    &class_id,
                    None,
                    class_context.to_native(),
                    None,
                    &mut results,
                )?,
            }
        };

        if results[0].hr.is_err() {
            return Err(OpcError::Com {
                source: results[0].hr.into(),
            });
        }

        match results[0].pItf.as_ref() {
            Some(itf) => itf
                .cast::<windows::core::IUnknown>()?
                .try_into()
                .map_err(|source| OpcError::Com { source }),
            None => Err(OpcError::Com {
                source: windows::core::Error::from(windows::Win32::Foundation::E_POINTER),
            }),
        }
    }
}
