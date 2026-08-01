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
    /// Create the `IOPCServerList` enumerator (local or remote DCOM). 提取自
    /// `get_servers` 供富信息枚举复用——`get_servers` 拿到 `GuidIterator` 后
    /// `IOPCServerList` 即 drop，而 `GetClassDetails` 需要持有 `IOPCServerList`。
    fn create_server_list(
        &self,
        server_info: Option<ServerInfo>,
    ) -> OpcResult<crate::bindings::comn::IOPCServerList> {
        tracing::debug!("Enumerating OPC DA Server classes via COM Component Categories Manager");
        let id = unsafe {
            windows::Win32::System::Com::CLSIDFromProgID(windows::core::w!("OPC.ServerList.1"))?
        };
        let servers = match server_info {
            // 非空 name（含 "localhost"）→ DCOM 路径；空 name/None → 本地 CoCreateInstance。
            // localhost 走 DCOM 绕过本地 in-proc 尝试，与 helpers::is_remote_host 一致。
            Some(info) if !info.name.is_empty() => {
                let has_credentials = !info.auth_info.auth_identity_data.user.is_empty();
                let bridge = info.into_bridge();
                let mut native = bridge.try_to_native()?;
                if !has_credentials {
                    // 空凭据 → DCOM 默认认证（当前登录用户），与历史行为一致。
                    native.pAuthInfo = std::ptr::null_mut();
                }
                let mut results = [windows::Win32::System::Com::MULTI_QI {
                    pIID: &crate::bindings::comn::IOPCServerList::IID,
                    pItf: core::mem::ManuallyDrop::new(None),
                    hr: windows::core::HRESULT(0),
                }];
                // SAFETY: `bridge` 持有 `native` 引用的全部内存，存活到本调用之后；
                // CoCreateInstanceEx with a remote COSERVERINFO instantiates IOPCServerList
                // on `info.name` and returns it via MULTI_QI.
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
        Ok(servers)
    }

    /// Retrieves an iterator over available server GUIDs.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `GuidIterator` over server GUIDs, or an error if the operation fails.
    fn get_servers(&self, server_info: Option<ServerInfo>) -> OpcResult<GuidIterator> {
        let servers = self.create_server_list(server_info)?;
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

        match server_info {
            Some(info) => {
                // 空用户名 → pAuthInfo:null（当前登录用户），保持向后兼容；
                // 非空 → 走 Bridge 用显式凭据（COAUTHINFO/COAUTHIDENTITY）。
                let has_credentials = !info.auth_info.auth_identity_data.user.is_empty();
                let bridge = info.into_bridge();
                let mut native = bridge.try_to_native()?;
                if !has_credentials {
                    native.pAuthInfo = std::ptr::null_mut();
                }
                // SAFETY: `bridge` 持有 `native` 引用的全部内存（name/AuthInfo 的
                // LocalPointer wide string + Box<COAUTH*>)，存活到本调用之后。
                unsafe {
                    windows::Win32::System::Com::CoCreateInstanceEx(
                        &class_id,
                        None,
                        class_context.to_native(),
                        Some(&native),
                        &mut results,
                    )?
                }
            }
            None => unsafe {
                windows::Win32::System::Com::CoCreateInstanceEx(
                    &class_id,
                    None,
                    class_context.to_native(),
                    None,
                    &mut results,
                )?
            },
        }

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
