//! Abstractions for OPC DA server connectivity.
//!
//! Defines the [`ServerConnector`], [`ConnectedServer`], and [`ConnectedGroup`]
//! traits that decouple [`super::opc_da::OpcDaClient`] from concrete COM types.
//! This enables mock implementations for unit testing without a live COM server.

pub use crate::bindings::da::tagOPCITEMDEF;
pub use crate::bindings::da::{tagOPCITEMRESULT, tagOPCITEMSTATE};
pub use crate::opc_da::client::*;
pub use crate::opc_da::com_utils::RemoteArray;
use crate::opc_da::com_utils::TryFromNative as _;
use crate::opc_da::com_utils::clear_variant_array;
pub use crate::opc_da::errors::{OpcError, OpcResult};
use crate::opc_da::typedefs::ServerStatus;
use crate::provider::{ItemProperty, TagValue};
pub use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface;

/// Factory for connecting to OPC DA servers.
///
/// Abstracts the concrete COM client usage so that tests can inject mocks
/// that return pre-configured server/group results without a live COM runtime.
///
/// # Errors
///
/// All methods return `OpcResult` — implementations should wrap COM errors
/// with contextual messages.
pub trait ServerConnector: Send + Sync {
    /// The server facade type returned by [`Self::connect`].
    type Server: ConnectedServer;

    /// Enumerate all OPC DA server ProgIDs on the local machine.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM registry enumeration fails.
    /// Enumerate OPC DA server ProgIDs on `host`.
    ///
    /// `host == "localhost"` (or empty) enumerates the local machine. A remote host requires
    /// DCOM `CoCreateInstanceEx` against the remote `IOPCServerList` (see ROADMAP P1-05:
    /// `ComConnector` currently enumerates locally regardless of `host`).
    fn enumerate_servers(&self, host: &str) -> OpcResult<Vec<String>>;

    /// Enumerate OPC DA servers with details (CLSID + user_type description).
    ///
    /// 默认实现退化为 prog_id only（clsid 空、user_type None）。`ComConnector` override
    /// 调 `IOPCServerList::GetClassDetails` 填充富信息。
    ///
    /// # Errors
    /// 同 [`enumerate_servers`](Self::enumerate_servers)。
    fn enumerate_servers_with_details(&self, host: &str) -> OpcResult<Vec<crate::ServerDesc>> {
        Ok(self
            .enumerate_servers(host)?
            .into_iter()
            .map(|prog_id| crate::ServerDesc {
                prog_id,
                clsid: String::new(),
                user_type: None,
            })
            .collect())
    }

    /// Connect to the named OPC DA server and return a server facade.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM server cannot be created or connected.
    fn connect(&self, server_name: &str) -> OpcResult<Self::Server>;
}

/// Facade over a connected OPC DA server instance.
///
/// Wraps namespace browsing and group management operations in Rust-native types.
///
/// # Errors
///
/// All methods return `OpcResult` — COM errors are propagated with context.
pub trait ConnectedServer {
    /// The group facade type returned by [`Self::add_group`].
    type Group: ConnectedGroup;

    /// Query the server's namespace organization type.
    ///
    /// Returns `OPC_NS_FLAT` or `OPC_NS_HIERARCHICAL` as a `u32`.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM call fails.
    fn query_organization(&self) -> OpcResult<u32>;

    /// Browse the server's address space for item IDs of the given type.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM browse call fails.
    fn browse_opc_item_ids(
        &self,
        browse_type: u32,
        filter: Option<&str>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<StringIterator>;

    /// Change the current browse position (e.g., navigate into/out of branches).
    ///
    /// # Errors
    ///
    /// Returns an error if the position change is rejected by the server.
    fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()>;

    /// Resolve a browse name to its fully-qualified item ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot resolve the item name.
    fn get_item_id(&self, item_name: &str) -> OpcResult<String>;

    /// Add a new OPC group to this server connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the group creation fails.
    #[allow(clippy::too_many_arguments)]
    fn add_group(
        &self,
        name: &str,
        active: bool,
        update_rate: u32,
        client_handle: GroupHandle,
        time_bias: i32,
        percent_deadband: f32,
        locale_id: u32,
        revised_update_rate: &mut u32,
        server_handle: &mut GroupHandle,
    ) -> OpcResult<Self::Group>;

    /// Remove an OPC group by its server-assigned handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the group removal fails.
    fn remove_group(&self, server_group: GroupHandle, force: bool) -> OpcResult<()>;

    /// Query the current server status.
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCServer::GetStatus` is unsupported or fails.
    fn get_status(&self) -> OpcResult<ServerStatus> {
        Err(OpcError::NotImplemented(
            "get_status not supported".to_string(),
        ))
    }

    /// Query the available properties of an item.
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCItemProperties` is unsupported or the query fails.
    fn get_item_properties(&self, _item_id: &str) -> OpcResult<Vec<ItemProperty>> {
        Err(OpcError::NotImplemented(
            "get_item_properties not supported".to_string(),
        ))
    }

    /// Get a server-localized error string for an HRESULT.
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCCommon` is unsupported or the call fails.
    fn get_error_string(&self, _hresult: i32) -> OpcResult<String> {
        Err(OpcError::NotImplemented(
            "get_error_string not supported".to_string(),
        ))
    }

    /// Advise an `IOPCShutdown` sink to this server, returning the connection cookie.
    ///
    /// # Errors
    ///
    /// Returns an error if the shutdown connection point is unavailable or `Advise` fails.
    fn advise_shutdown(&self, _sink: &windows::core::IUnknown) -> OpcResult<u32> {
        Err(OpcError::NotImplemented(
            "advise_shutdown not supported".to_string(),
        ))
    }

    /// Unadvise a previously advised shutdown sink by cookie.
    ///
    /// # Errors
    ///
    /// Returns an error if the cookie is unknown or `Unadvise` fails.
    fn unadvise_shutdown(&self, _cookie: u32) -> OpcResult<()> {
        Err(OpcError::NotImplemented(
            "unadvise_shutdown not supported".to_string(),
        ))
    }

    /// Set the server locale (`IOPCCommon::SetLocaleID`).
    fn set_locale_id(&self, _locale_id: u32) -> OpcResult<()> {
        Err(OpcError::NotImplemented(
            "set_locale_id not supported".to_string(),
        ))
    }

    /// Set the client application name (`IOPCCommon::SetClientName`).
    fn set_client_name(&self, _name: &str) -> OpcResult<()> {
        Err(OpcError::NotImplemented(
            "set_client_name not supported".to_string(),
        ))
    }
}

/// Facade over an OPC DA group for item management and I/O.
///
/// # Errors
///
/// All methods return `OpcResult` — COM errors are propagated with context.
pub trait ConnectedGroup {
    /// Add items to this group for monitoring.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `AddItems` call fails.
    fn add_items(
        &self,
        items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )>;

    /// Perform a synchronous read of the given server handles.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `Read` call fails.
    fn read(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMSTATE>,
        RemoteArray<windows::core::HRESULT>,
    )>;

    /// Write values to the given server handles.
    ///
    /// # Errors
    ///
    /// Returns an error if the COM `Write` call fails.
    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>>;

    /// Read item values with a maximum-age constraint, returning assembled `TagValue`s.
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCSyncIO2` is unsupported or the read fails.
    fn read_max_age(
        &self,
        _server_handles: &[ItemHandle],
        _max_age_ms: u32,
        _tag_ids: &[String],
    ) -> OpcResult<Vec<TagValue>> {
        Err(OpcError::NotImplemented(
            "read_max_age not supported".to_string(),
        ))
    }

    /// Write values with quality/timestamp (`IOPCSyncIO2::WriteVQT`).
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCSyncIO2` is unsupported or the write fails.
    fn write_vqt(
        &self,
        _server_handles: &[ItemHandle],
        _values: &[crate::bindings::da::tagOPCITEMVQT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        Err(OpcError::NotImplemented(
            "write_vqt not supported".to_string(),
        ))
    }

    /// Advise an `IOPCDataCallback` sink to this group, returning the connection cookie.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection point is unavailable or `Advise` fails.
    fn advise_data_callback(&self, _sink: &windows::core::IUnknown) -> OpcResult<u32> {
        Err(OpcError::NotImplemented(
            "advise_data_callback not supported".to_string(),
        ))
    }

    /// Unadvise a previously advised callback by cookie.
    ///
    /// # Errors
    ///
    /// Returns an error if the cookie is unknown or `Unadvise` fails.
    fn unadvise_data_callback(&self, _cookie: u32) -> OpcResult<()> {
        Err(OpcError::NotImplemented(
            "unadvise_data_callback not supported".to_string(),
        ))
    }

    /// Adjust the group's update rate at runtime (`IOPCGroupStateMgt::SetState`).
    ///
    /// # Errors
    ///
    /// Returns an error if `SetState` is unsupported or fails.
    fn set_update_rate(&self, _update_rate: u32) -> OpcResult<u32> {
        Err(OpcError::NotImplemented(
            "set_update_rate not supported".to_string(),
        ))
    }

    /// Set the group keep-alive interval (`IOPCGroupStateMgt2::SetKeepAlive`).
    ///
    /// # Errors
    ///
    /// Returns an error if `IOPCGroupStateMgt2` is unsupported or the call fails.
    fn set_keep_alive(&self, _keep_alive_ms: u32) -> OpcResult<u32> {
        Err(OpcError::NotImplemented(
            "set_keep_alive not supported".to_string(),
        ))
    }
}

// ── COM-backed implementations ──────────────────────────────────────

/// Real COM-backed server connector implementation.
///
/// Uses Windows COM to enumerate and connect to OPC DA servers. Carries a `host` that selects
/// the target machine for [`Self::connect`]: remote hosts go through `create_server2` (DCOM).
pub struct ComConnector {
    host: String,
    credentials: Option<crate::opc_da::typedefs::AuthCredentials>,
}

impl ComConnector {
    /// Create a connector targeting `host` (e.g. `"localhost"` or `"192.168.1.10"`),
    /// using the current logged-in user's credentials (DCOM default authentication).
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            credentials: None,
        }
    }

    /// Create a connector targeting `host` authenticated with explicit DCOM credentials
    /// (`user`/`password`/`domain`). Use this when the current logged-in user cannot
    /// access the remote OPC DA Server (cross-domain, dedicated service account, etc.).
    #[must_use]
    pub fn with_credentials(
        host: impl Into<String>,
        credentials: crate::opc_da::typedefs::AuthCredentials,
    ) -> Self {
        Self {
            host: host.into(),
            credentials: Some(credentials),
        }
    }

    /// Target host for subsequent `connect` calls.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Default for ComConnector {
    fn default() -> Self {
        Self::new("localhost")
    }
}

impl ServerConnector for ComConnector {
    type Server = ComServer;

    fn enumerate_servers(&self, host: &str) -> OpcResult<Vec<String>> {
        let client = crate::opc_da::client::v2::Client;
        let auth_info = self.credentials.as_ref().map_or_else(
            crate::opc_da::typedefs::AuthInfo::default_dcom,
            crate::opc_da::typedefs::AuthCredentials::to_auth_info,
        );
        let server_info = crate::opc_da::typedefs::ServerInfo {
            name: host.to_string(),
            auth_info,
        };
        let guid_iter = client.get_servers(Some(server_info)).map_err(|e| {
            tracing::warn!(error = ?e, host = %host, "IOPCServerList enumeration failed");
            e
        })?;

        let mut servers = Vec::new();
        for guid in guid_iter.flatten() {
            // SAFETY: `crate::opc_da::GUID` and `windows::core::GUID` are both
            // `#[repr(C)]` structs with identical layout (4-byte, 2-byte, 2-byte,
            // 8-byte array). This is validated by a `const_assert_eq!` in
            // `opc_da/client/iterator.rs`.
            let win_guid: windows::core::GUID = unsafe { std::mem::transmute_copy(&guid) };
            if win_guid == windows::core::GUID::zeroed() {
                continue;
            }

            if let Ok(progid) = crate::helpers::guid_to_progid(&win_guid)
                && !progid.is_empty()
            {
                servers.push(progid);
            }
        }
        servers.sort();
        servers.dedup();
        Ok(servers)
    }

    /// 富信息枚举：持 `IOPCServerList`，每个 CLSID 调 `GetClassDetails` 取 ProgID +
    /// UserType 描述。GetClassDetails 失败时退化为 `guid_to_progid`（prog_id only）。
    fn enumerate_servers_with_details(&self, host: &str) -> OpcResult<Vec<crate::ServerDesc>> {
        let client = crate::opc_da::client::v2::Client;
        let auth_info = self.credentials.as_ref().map_or_else(
            crate::opc_da::typedefs::AuthInfo::default_dcom,
            crate::opc_da::typedefs::AuthCredentials::to_auth_info,
        );
        let server_info = crate::opc_da::typedefs::ServerInfo {
            name: host.to_string(),
            auth_info,
        };
        let server_list = client.create_server_list(Some(server_info)).map_err(|e| {
            tracing::warn!(error = ?e, host = %host, "IOPCServerList creation failed");
            e
        })?;

        let versions = [crate::opc_da::client::v2::Client::CATALOG_ID];
        // SAFETY: EnumClassesOfCategories 按 OPC DA CATID 枚举 server CLSID。
        let iter = unsafe {
            server_list
                .EnumClassesOfCategories(&versions, &versions)
                .map_err(|e| {
                    windows::core::Error::new(e.code(), "Failed to enumerate server classes")
                })?
        };
        let guid_iter = crate::opc_da::client::GuidIterator::new(iter);

        let mut servers: Vec<crate::ServerDesc> = Vec::new();
        for guid in guid_iter.flatten() {
            // SAFETY: `crate::opc_da::GUID` 与 `windows::core::GUID` 布局一致
            //（const_assert_eq! in client/iterator.rs）。
            let win_guid: windows::core::GUID = unsafe { std::mem::transmute_copy(&guid) };
            if win_guid == windows::core::GUID::zeroed() {
                continue;
            }
            // windows 0.61 的 GUID 未 impl Display，手动格式化标准 CLSID 字符串。
            let clsid = format!(
                "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                win_guid.data1,
                win_guid.data2,
                win_guid.data3,
                win_guid.data4[0],
                win_guid.data4[1],
                win_guid.data4[2],
                win_guid.data4[3],
                win_guid.data4[4],
                win_guid.data4[5],
                win_guid.data4[6],
                win_guid.data4[7],
            );

            // GetClassDetails → ProgID + UserType（2 个 PWSTR，COM allocator 分配，用完释放）。
            let mut progid_ptr = windows::core::PWSTR::null();
            let mut usertype_ptr = windows::core::PWSTR::null();
            // SAFETY: `win_guid` 是有效 CLSID（枚举所得）；`progid_ptr`/`usertype_ptr`
            // 是本地 PWSTR（null 初始化），GetClassDetails 用 COM allocator 写入。
            let details_ok = unsafe {
                server_list
                    .GetClassDetails(
                        &raw const win_guid,
                        &raw mut progid_ptr,
                        &raw mut usertype_ptr,
                    )
                    .is_ok()
            };
            let (prog_id, user_type) = if details_ok {
                // SAFETY: 两 PWSTR 由 GetClassDetails 用 COM allocator 分配；`to_string`
                // 读取（unsafe，读 null-terminated wide string）后 CoTaskMemFree 释放。
                unsafe {
                    let prog = progid_ptr.to_string().unwrap_or_default();
                    let user = usertype_ptr.to_string().unwrap_or_default();
                    if !progid_ptr.is_null() {
                        windows::Win32::System::Com::CoTaskMemFree(Some(
                            progid_ptr.as_ptr().cast(),
                        ));
                    }
                    if !usertype_ptr.is_null() {
                        windows::Win32::System::Com::CoTaskMemFree(Some(
                            usertype_ptr.as_ptr().cast(),
                        ));
                    }
                    (prog, user)
                }
            } else {
                // GetClassDetails 失败 → 退化为 guid_to_progid（prog_id only）。
                let prog = crate::helpers::guid_to_progid(&win_guid).unwrap_or_default();
                (prog, String::new())
            };

            if prog_id.is_empty() {
                continue;
            }
            servers.push(crate::ServerDesc {
                prog_id,
                clsid,
                user_type: if user_type.is_empty() {
                    None
                } else {
                    Some(user_type)
                },
            });
        }
        servers.sort_by(|a, b| a.prog_id.cmp(&b.prog_id));
        servers.dedup_by(|a, b| a.prog_id == b.prog_id);
        Ok(servers)
    }

    fn connect(&self, server_name: &str) -> OpcResult<Self::Server> {
        let opc_server =
            crate::helpers::connect_server(server_name, &self.host, self.credentials.as_ref())?;
        let unknown: windows::core::IUnknown = opc_server.cast()?;

        Ok(ComServer {
            server: opc_server,
            common: unknown.cast()?,
            connection_point_container: unknown.cast()?,
            item_properties: unknown.cast().ok(),
            browse: unknown.cast().ok(),
            item_io: unknown.cast().ok(),
            server_public_groups: unknown.cast().ok(),
            browse_server_address_space: unknown.cast().ok(),
        })
    }
}

/// COM-backed [`ConnectedServer`].
pub struct ComServer {
    pub(crate) server: crate::bindings::da::IOPCServer,
    pub(crate) common: crate::bindings::comn::IOPCCommon,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) item_properties: Option<crate::bindings::da::IOPCItemProperties>,
    pub(crate) browse: Option<crate::bindings::da::IOPCBrowse>,
    pub(crate) item_io: Option<crate::bindings::da::IOPCItemIO>,
    pub(crate) server_public_groups: Option<crate::bindings::da::IOPCServerPublicGroups>,
    pub(crate) browse_server_address_space:
        Option<crate::bindings::da::IOPCBrowseServerAddressSpace>,
}

impl ServerTrait<ComGroup> for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServer> {
        Ok(&self.server)
    }
}

impl CommonTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::comn::IOPCCommon> {
        Ok(&self.common)
    }
}

impl ConnectionPointContainerTrait for ComServer {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl ItemPropertiesTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemProperties> {
        self.item_properties
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCItemProperties not supported".to_string()))
    }
}

impl ServerPublicGroupsTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCServerPublicGroups> {
        self.server_public_groups.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCServerPublicGroups not supported".to_string())
        })
    }
}

impl BrowseServerAddressSpaceTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCBrowseServerAddressSpace> {
        self.browse_server_address_space.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCBrowseServerAddressSpace not supported".to_string())
        })
    }
}

impl BrowseTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCBrowse> {
        self.browse
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCBrowse not supported".to_string()))
    }
}

impl ItemIoTrait for ComServer {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemIO> {
        self.item_io
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCItemIO not supported".to_string()))
    }
}

impl ConnectedServer for ComServer {
    type Group = ComGroup;

    fn query_organization(&self) -> OpcResult<u32> {
        let org = BrowseServerAddressSpaceTrait::query_organization(self)?;
        Ok(org.0.cast_unsigned())
    }

    fn browse_opc_item_ids(
        &self,
        browse_type: u32,
        filter: Option<&str>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<StringIterator> {
        BrowseServerAddressSpaceTrait::browse_opc_item_ids(
            self,
            crate::bindings::da::tagOPCBROWSETYPE(browse_type.cast_signed()),
            filter,
            data_type,
            access_rights,
        )
    }

    fn change_browse_position(&self, direction: u32, name: &str) -> OpcResult<()> {
        BrowseServerAddressSpaceTrait::change_browse_position(
            self,
            crate::bindings::da::tagOPCBROWSEDIRECTION(direction.cast_signed()),
            name,
        )
    }

    fn get_item_id(&self, item_name: &str) -> OpcResult<String> {
        BrowseServerAddressSpaceTrait::get_item_id(self, item_name)
    }

    fn add_group(
        &self,
        name: &str,
        active: bool,
        update_rate: u32,
        client_handle: GroupHandle,
        time_bias: i32,
        percent_deadband: f32,
        locale_id: u32,
        revised_update_rate: &mut u32,
        server_handle: &mut GroupHandle,
    ) -> OpcResult<Self::Group> {
        ServerTrait::add_group(
            self,
            name,
            active,
            update_rate,
            client_handle,
            time_bias,
            percent_deadband,
            locale_id,
            revised_update_rate,
            server_handle,
        )
    }

    fn remove_group(&self, server_group: GroupHandle, force: bool) -> OpcResult<()> {
        ServerTrait::remove_group(self, server_group, force)
    }

    fn get_status(&self) -> OpcResult<ServerStatus> {
        let ptr = ServerTrait::get_status(self)?;
        let native = ptr.ok()?;
        ServerStatus::try_from_native(native).map_err(OpcError::from)
    }

    fn get_item_properties(&self, item_id: &str) -> OpcResult<Vec<ItemProperty>> {
        use crate::opc_da::com_utils::TryFromNative as _;

        let (ids, descriptions, datatypes) =
            ItemPropertiesTrait::query_available_properties(self, item_id)?;
        let ids_slice = ids.as_slice();
        if ids_slice.is_empty() {
            return Ok(Vec::new());
        }
        let (mut values, errors) =
            ItemPropertiesTrait::get_item_properties(self, item_id, ids_slice)?;
        let descs = descriptions.as_slice();
        let dtypes = datatypes.as_slice();
        let vals = values.as_slice();
        let errs = errors.as_slice();

        let mut out = Vec::with_capacity(ids_slice.len());
        for (i, &id) in ids_slice.iter().enumerate() {
            let description = descs
                .get(i)
                .and_then(|p| String::try_from_native(p).ok())
                .unwrap_or_default();
            let data_type = *dtypes.get(i).unwrap_or(&0);
            let value = if errs.get(i).is_some_and(|e| e.is_ok()) {
                vals.get(i)
                    .map(crate::helpers::variant_to_string)
                    .unwrap_or_default()
            } else {
                let hr = errs.get(i).copied().unwrap_or(windows::core::HRESULT(0));
                format!("Error: {}", crate::helpers::format_hresult(hr))
            };
            out.push(ItemProperty {
                id,
                description,
                data_type,
                value,
            });
        }
        clear_variant_array(&mut values);
        Ok(out)
    }

    fn get_error_string(&self, hresult: i32) -> OpcResult<String> {
        CommonTrait::get_error_string(self, windows::core::HRESULT(hresult))
    }

    fn advise_shutdown(&self, sink: &windows::core::IUnknown) -> OpcResult<u32> {
        let cp = ConnectionPointContainerTrait::find_connection_point(
            self,
            &crate::bindings::comn::IOPCShutdown::IID,
        )?;
        // SAFETY: `sink` implements IOPCShutdown (verified by the connection point).
        let cookie = unsafe { cp.Advise(sink)? };
        Ok(cookie)
    }

    fn unadvise_shutdown(&self, cookie: u32) -> OpcResult<()> {
        let cp = ConnectionPointContainerTrait::find_connection_point(
            self,
            &crate::bindings::comn::IOPCShutdown::IID,
        )?;
        // SAFETY: `cookie` was returned by a prior `Advise` on this connection point.
        unsafe {
            cp.Unadvise(cookie)?;
        }
        Ok(())
    }

    fn set_locale_id(&self, locale_id: u32) -> OpcResult<()> {
        CommonTrait::set_locale_id(self, locale_id)
    }

    fn set_client_name(&self, name: &str) -> OpcResult<()> {
        CommonTrait::set_client_name(self, name)
    }
}

pub struct ComGroup {
    pub(crate) item_mgt: crate::bindings::da::IOPCItemMgt,
    pub(crate) group_state_mgt: crate::bindings::da::IOPCGroupStateMgt,
    pub(crate) public_group_state_mgt: Option<crate::bindings::da::IOPCPublicGroupStateMgt>,
    pub(crate) sync_io: crate::bindings::da::IOPCSyncIO,
    pub(crate) sync_io2: Option<crate::bindings::da::IOPCSyncIO2>,
    pub(crate) group_state_mgt2: Option<crate::bindings::da::IOPCGroupStateMgt2>,
    pub(crate) async_io3: Option<crate::bindings::da::IOPCAsyncIO3>,
    pub(crate) item_deadband_mgt: Option<crate::bindings::da::IOPCItemDeadbandMgt>,
    pub(crate) item_sampling_mgt: Option<crate::bindings::da::IOPCItemSamplingMgt>,
    pub(crate) async_io: Option<crate::bindings::da::IOPCAsyncIO>,
    pub(crate) async_io2: crate::bindings::da::IOPCAsyncIO2,
    pub(crate) connection_point_container: windows::Win32::System::Com::IConnectionPointContainer,
    pub(crate) data_object: Option<windows::Win32::System::Com::IDataObject>,
}

impl ItemMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemMgt> {
        Ok(&self.item_mgt)
    }
}

impl GroupStateMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCGroupStateMgt> {
        Ok(&self.group_state_mgt)
    }
}

impl PublicGroupStateMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCPublicGroupStateMgt> {
        self.public_group_state_mgt.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCPublicGroupStateMgt not supported".to_string())
        })
    }
}

impl SyncIoTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO> {
        Ok(&self.sync_io)
    }
}

impl SyncIo2Trait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCSyncIO2> {
        self.sync_io2
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCSyncIO2 not supported".to_string()))
    }
}

impl GroupStateMgt2Trait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCGroupStateMgt2> {
        self.group_state_mgt2
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCGroupStateMgt2 not supported".to_string()))
    }
}

impl AsyncIo3Trait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO3> {
        self.async_io3
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCAsyncIO3 not supported".to_string()))
    }
}

impl ItemDeadbandMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemDeadbandMgt> {
        self.item_deadband_mgt.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCItemDeadbandMgt not supported".to_string())
        })
    }
}

impl ItemSamplingMgtTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCItemSamplingMgt> {
        self.item_sampling_mgt.as_ref().ok_or_else(|| {
            OpcError::NotImplemented("IOPCItemSamplingMgt not supported".to_string())
        })
    }
}

impl AsyncIoTrait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO> {
        self.async_io
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IOPCAsyncIO not supported".to_string()))
    }
}

impl AsyncIo2Trait for ComGroup {
    fn interface(&self) -> OpcResult<&crate::bindings::da::IOPCAsyncIO2> {
        Ok(&self.async_io2)
    }
}

impl ConnectionPointContainerTrait for ComGroup {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IConnectionPointContainer> {
        Ok(&self.connection_point_container)
    }
}

impl DataObjectTrait for ComGroup {
    fn interface(&self) -> OpcResult<&windows::Win32::System::Com::IDataObject> {
        self.data_object
            .as_ref()
            .ok_or_else(|| OpcError::NotImplemented("IDataObject not supported".to_string()))
    }
}

impl ConnectedGroup for ComGroup {
    fn add_items(
        &self,
        items: &[tagOPCITEMDEF],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMRESULT>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        ItemMgtTrait::add_items(self, items)
    }

    fn read(
        &self,
        source: crate::bindings::da::tagOPCDATASOURCE,
        server_handles: &[ItemHandle],
    ) -> OpcResult<(
        RemoteArray<tagOPCITEMSTATE>,
        RemoteArray<windows::core::HRESULT>,
    )> {
        SyncIoTrait::read(self, source, server_handles)
    }

    fn write(
        &self,
        server_handles: &[ItemHandle],
        values: &[VARIANT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        SyncIoTrait::write(self, server_handles, values)
    }

    fn read_max_age(
        &self,
        server_handles: &[ItemHandle],
        max_age_ms: u32,
        tag_ids: &[String],
    ) -> OpcResult<Vec<TagValue>> {
        let n = server_handles.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let max_ages = vec![max_age_ms; n];
        let (mut values, qualities, timestamps, errors) =
            SyncIo2Trait::read_max_age(self, server_handles, &max_ages)?;
        let vals = values.as_slice();
        let quals = qualities.as_slice();
        let times = timestamps.as_slice();
        let errs = errors.as_slice();

        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let tag_id = tag_ids.get(i).cloned().unwrap_or_default();
            let (value, quality, data_type) = if errs.get(i).is_some_and(|e| e.is_ok()) {
                let v = vals
                    .get(i)
                    .map_or_else(|| "Error".to_string(), crate::helpers::variant_to_string);
                let dt = vals
                    .get(i)
                    .map_or_else(String::new, crate::helpers::variant_type_name);
                let q = quals.get(i).map_or_else(
                    || "Bad".to_string(),
                    |&q| crate::helpers::quality_to_string(q),
                );
                (v, q, dt)
            } else {
                ("Error".to_string(), "Bad".to_string(), String::new())
            };
            let timestamp = times
                .get(i)
                .map(|&ft| crate::helpers::filetime_to_string(ft))
                .unwrap_or_default();
            out.push(TagValue {
                tag_id,
                value,
                data_type,
                quality,
                timestamp,
            });
        }
        clear_variant_array(&mut values);
        Ok(out)
    }

    fn write_vqt(
        &self,
        server_handles: &[ItemHandle],
        values: &[crate::bindings::da::tagOPCITEMVQT],
    ) -> OpcResult<RemoteArray<windows::core::HRESULT>> {
        SyncIo2Trait::write_vqt(self, server_handles, values)
    }

    fn advise_data_callback(&self, sink: &windows::core::IUnknown) -> OpcResult<u32> {
        let cp = ConnectionPointContainerTrait::data_callback_connection_point(self)?;
        // SAFETY: `sink` implements IOPCDataCallback (verified by the connection point).
        let cookie = unsafe { cp.Advise(sink)? };
        Ok(cookie)
    }

    fn unadvise_data_callback(&self, cookie: u32) -> OpcResult<()> {
        let cp = ConnectionPointContainerTrait::data_callback_connection_point(self)?;
        // SAFETY: `cookie` was returned by a prior `Advise` on this connection point.
        unsafe {
            cp.Unadvise(cookie)?;
        }
        Ok(())
    }

    fn set_update_rate(&self, update_rate: u32) -> OpcResult<u32> {
        GroupStateMgtTrait::set_state(self, Some(update_rate), None, None, None, None, None)
    }

    fn set_keep_alive(&self, keep_alive_ms: u32) -> OpcResult<u32> {
        GroupStateMgt2Trait::set_keep_alive(self, keep_alive_ms)
    }
}

impl TryFrom<windows::core::IUnknown> for ComGroup {
    type Error = windows::core::Error;

    fn try_from(unknown: windows::core::IUnknown) -> Result<Self, Self::Error> {
        Ok(Self {
            item_mgt: unknown.cast()?,
            group_state_mgt: unknown.cast()?,
            public_group_state_mgt: unknown.cast().ok(),
            sync_io: unknown.cast()?,
            sync_io2: unknown.cast().ok(),
            group_state_mgt2: unknown.cast().ok(),
            async_io3: unknown.cast().ok(),
            item_deadband_mgt: unknown.cast().ok(),
            item_sampling_mgt: unknown.cast().ok(),
            async_io: unknown.cast().ok(),
            async_io2: unknown.cast()?,
            connection_point_container: unknown.cast()?,
            data_object: unknown.cast().ok(),
        })
    }
}
