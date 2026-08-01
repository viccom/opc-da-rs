use crate::opc_da::com_utils::{
    IntoBridge, LocalPointer, RemoteArray, ToNative, TryFromNative, TryToNative,
};
use crate::try_from_native;

/// Opaque handle for an OPC group.
///
/// This wrapper type enhances type safety when interacting with OPC COM interfaces,
/// preventing accidental mixing of group and item handles.
///
/// # Examples
///
/// ```
/// use opc_da_client::GroupHandle;
/// let handle = GroupHandle(123u32);
/// assert_eq!(handle.0, 123u32);
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GroupHandle(pub u32);

/// Opaque handle for an OPC item.
///
/// Similar to [`GroupHandle`], this ensures type-safe identification of tags
/// within an OPC group.
///
/// # Examples
///
/// ```
/// use opc_da_client::ItemHandle;
/// let handle = ItemHandle(456u32);
/// assert_eq!(handle.0, 456u32);
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ItemHandle(pub u32);

/// Supported OPC DA Specification versions.
#[derive(Debug, Clone, PartialEq)]
pub enum Version {
    V1,
    V2,
    V3,
}

/// Current state and properties of an active OPC group.
///
/// This structure encapsulates both the requested and currently active properties
/// of an OPC group, as reported by the server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupState {
    /// Actual update rate in milliseconds (may differ from requested).
    pub update_rate: u32,
    /// Whether the group is currently active (processing updates).
    pub active: bool,
    /// The unique name of the group.
    pub name: String,
    /// Time zone bias in minutes from UTC.
    pub time_bias: i32,
    /// Percent change for a tag value required to trigger an update.
    pub percent_deadband: f32,
    /// Locale ID used for formatting strings in this group.
    pub locale_id: u32,
    /// Handle assigned by the client for this group.
    pub client_handle: GroupHandle,
    /// Handle assigned by the server for this group.
    pub server_handle: GroupHandle,
}

/// Operational status and metadata of the connected server.
///
/// This structure provides a snapshot of the server's health, current load,
/// and version information.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    /// Time when the server was started.
    pub start_time: std::time::SystemTime,
    /// Current time according to the server.
    pub current_time: std::time::SystemTime,
    /// Time of the last data update.
    pub last_update_time: std::time::SystemTime,
    /// The current operational state of the server.
    pub server_state: ServerState,
    /// Number of groups currently managed by the server.
    pub group_count: u32,
    /// Current bandwidth utilization as reported by the server.
    pub band_width: u32,
    /// Major version of the server software.
    pub major_version: u16,
    /// Minor version of the server software.
    pub minor_version: u16,
    /// Build or revision number of the server software.
    pub build_number: u16,
    /// Descriptive vendor-specific information.
    pub vendor_info: String,
}

impl TryFromNative<crate::bindings::da::tagOPCSERVERSTATUS> for ServerStatus {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCSERVERSTATUS,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            start_time: try_from_native!(&native.ftStartTime),
            current_time: try_from_native!(&native.ftCurrentTime),
            last_update_time: try_from_native!(&native.ftLastUpdateTime),
            server_state: try_from_native!(&native.dwServerState),
            group_count: native.dwGroupCount,
            band_width: native.dwBandWidth,
            major_version: native.wMajorVersion,
            minor_version: native.wMinorVersion,
            build_number: native.wBuildNumber,
            vendor_info: try_from_native!(&native.szVendorInfo),
        })
    }
}

/// Definition required to add a new item to an OPC group.
///
/// This structure contains the parameters needed for the server to identify
/// and initialize a tag within a group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemDef {
    /// Optional access path for the item (server-specific).
    pub access_path: String,
    /// The unique identifier of the tag within the server namespace.
    pub item_id: String,
    /// Whether the item should be added in an active state.
    pub active: bool,
    /// Handle assigned by the client for this item.
    pub client_handle: ItemHandle,
    /// Requested canonical data type (0 for server default).
    pub data_type: u16,
    /// Optional opaque blob for the item.
    pub blob: Vec<u8>,
}

/// FFI-safe bridge struct for `ItemDef`.
pub struct ItemDefBridge {
    pub access_path: LocalPointer<Vec<u16>>,
    pub item_id: LocalPointer<Vec<u16>>,
    pub active: bool,
    pub item_client_handle: u32,
    pub requested_data_type: u16,
    pub blob: LocalPointer<Vec<u8>>,
}

impl IntoBridge<ItemDefBridge> for ItemDef {
    fn into_bridge(self) -> ItemDefBridge {
        ItemDefBridge {
            access_path: LocalPointer::from(&self.access_path),
            item_id: LocalPointer::from(&self.item_id),
            active: self.active,
            item_client_handle: self.client_handle.0,
            requested_data_type: self.data_type,
            blob: LocalPointer::new(Some(self.blob)),
        }
    }
}

impl TryToNative<crate::bindings::da::tagOPCITEMDEF> for ItemDefBridge {
    fn try_to_native(&self) -> windows::core::Result<crate::bindings::da::tagOPCITEMDEF> {
        Ok(crate::bindings::da::tagOPCITEMDEF {
            szAccessPath: self.access_path.as_pwstr(),
            szItemID: self.item_id.as_pwstr(),
            bActive: self.active.into(),
            hClient: self.item_client_handle,
            vtRequestedDataType: self.requested_data_type,
            dwBlobSize: self.blob.len().try_into().map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "Blob size exceeds u32 maximum value",
                )
            })?,
            pBlob: self.blob.as_array_ptr() as *mut _,
            wReserved: 0,
        })
    }
}

/// Result properties of an item after being added to a group.
///
/// This structure contains the server-assigned properties for an item
/// that was successfully added.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemResult {
    /// Handle assigned by the server for this item.
    pub server_handle: ItemHandle,
    /// The actual canonical data type supported by the server for this item.
    pub data_type: u16,
    /// Access rights for this item (read/write permissions).
    pub access_rights: u32,
    /// Optional opaque blob returned by the server.
    pub blob: Vec<u8>,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMRESULT> for ItemResult {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMRESULT,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            server_handle: ItemHandle(native.hServer),
            data_type: native.vtCanonicalDataType,
            access_rights: native.dwAccessRights,
            blob: RemoteArray::from_mut_ptr(native.pBlob, native.dwBlobSize)
                .as_slice()
                .to_vec(),
        })
    }
}

/// Current running state of the OPC server.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    Running,
    Failed,
    NoConfig,
    Suspended,
    Test,
    CommunicationFault,
}

impl TryFromNative<crate::bindings::da::tagOPCSERVERSTATE> for ServerState {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCSERVERSTATE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_STATUS_RUNNING => Ok(ServerState::Running),
            crate::bindings::da::OPC_STATUS_FAILED => Ok(ServerState::Failed),
            crate::bindings::da::OPC_STATUS_NOCONFIG => Ok(ServerState::NoConfig),
            crate::bindings::da::OPC_STATUS_SUSPENDED => Ok(ServerState::Suspended),
            crate::bindings::da::OPC_STATUS_TEST => Ok(ServerState::Test),
            crate::bindings::da::OPC_STATUS_COMM_FAULT => Ok(ServerState::CommunicationFault),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown server state: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCSERVERSTATE> for ServerState {
    fn to_native(&self) -> crate::bindings::da::tagOPCSERVERSTATE {
        match self {
            ServerState::Running => crate::bindings::da::OPC_STATUS_RUNNING,
            ServerState::Failed => crate::bindings::da::OPC_STATUS_FAILED,
            ServerState::NoConfig => crate::bindings::da::OPC_STATUS_NOCONFIG,
            ServerState::Suspended => crate::bindings::da::OPC_STATUS_SUSPENDED,
            ServerState::Test => crate::bindings::da::OPC_STATUS_TEST,
            ServerState::CommunicationFault => crate::bindings::da::OPC_STATUS_COMM_FAULT,
        }
    }
}

/// Scope for enumerating server items or connections.
#[derive(Debug, Clone, PartialEq)]
pub enum EnumScope {
    PrivateConnections,
    PublicConnections,
    AllConnections,
    Public,
    Private,
    All,
}

impl TryFromNative<crate::bindings::da::tagOPCENUMSCOPE> for EnumScope {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCENUMSCOPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_ENUM_PRIVATE_CONNECTIONS => Ok(EnumScope::PrivateConnections),
            crate::bindings::da::OPC_ENUM_PUBLIC_CONNECTIONS => Ok(EnumScope::PublicConnections),
            crate::bindings::da::OPC_ENUM_ALL_CONNECTIONS => Ok(EnumScope::AllConnections),
            crate::bindings::da::OPC_ENUM_PUBLIC => Ok(EnumScope::Public),
            crate::bindings::da::OPC_ENUM_PRIVATE => Ok(EnumScope::Private),
            crate::bindings::da::OPC_ENUM_ALL => Ok(EnumScope::All),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown enum scope: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCENUMSCOPE> for EnumScope {
    fn to_native(&self) -> crate::bindings::da::tagOPCENUMSCOPE {
        match self {
            EnumScope::PrivateConnections => crate::bindings::da::OPC_ENUM_PRIVATE_CONNECTIONS,
            EnumScope::PublicConnections => crate::bindings::da::OPC_ENUM_PUBLIC_CONNECTIONS,
            EnumScope::AllConnections => crate::bindings::da::OPC_ENUM_ALL_CONNECTIONS,
            EnumScope::Public => crate::bindings::da::OPC_ENUM_PUBLIC,
            EnumScope::Private => crate::bindings::da::OPC_ENUM_PRIVATE,
            EnumScope::All => crate::bindings::da::OPC_ENUM_ALL,
        }
    }
}

/// Full attribute set of a single OPC item.
pub struct ItemAttributes {
    pub access_path: String,
    pub item_id: String,
    pub active: bool,
    pub client_handle: ItemHandle,
    pub server_handle: ItemHandle,
    pub access_rights: u32,
    pub blob: Vec<u8>,
    pub requested_data_type: u16,
    pub canonical_data_type: u16,
    pub eu_type: EuType,
    pub eu_info: windows::Win32::System::Variant::VARIANT,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMATTRIBUTES> for ItemAttributes {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMATTRIBUTES,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            access_path: try_from_native!(&native.szAccessPath),
            item_id: try_from_native!(&native.szItemID),
            active: native.bActive.into(),
            client_handle: ItemHandle(native.hClient),
            server_handle: ItemHandle(native.hServer),
            access_rights: native.dwAccessRights,
            blob: RemoteArray::from_mut_ptr(native.pBlob, native.dwBlobSize)
                .as_slice()
                .to_vec(),
            requested_data_type: native.vtRequestedDataType,
            canonical_data_type: native.vtCanonicalDataType,
            eu_type: try_from_native!(&native.dwEUType),
            eu_info: native.vEUInfo.clone(),
        })
    }
}

/// Engineering Units (EU) classification type.
pub enum EuType {
    NoEnum,
    Analog,
    Enumerated,
}

impl TryFromNative<crate::bindings::da::tagOPCEUTYPE> for EuType {
    fn try_from_native(native: &crate::bindings::da::tagOPCEUTYPE) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_NOENUM => Ok(EuType::NoEnum),
            crate::bindings::da::OPC_ANALOG => Ok(EuType::Analog),
            crate::bindings::da::OPC_ENUMERATED => Ok(EuType::Enumerated),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown EU type: {unknown:?}"),
            )),
        }
    }
}

/// Current state of a watched OPC item including value, quality, and time.
pub struct ItemState {
    pub client_handle: ItemHandle,
    pub timestamp: std::time::SystemTime,
    pub quality: u16,
    pub data_value: windows::Win32::System::Variant::VARIANT,
}

impl TryFromNative<crate::bindings::da::tagOPCITEMSTATE> for ItemState {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCITEMSTATE,
    ) -> windows::core::Result<Self> {
        Ok(Self {
            client_handle: ItemHandle(native.hClient),
            timestamp: try_from_native!(&native.ftTimeStamp),
            quality: native.wQuality,
            data_value: native.vDataValue.clone(),
        })
    }
}

/// Reading source preference (Cache or Device).
pub enum DataSourceTarget {
    ForceCache,
    ForceDevice,
    WithMaxAge(u32),
}

impl DataSourceTarget {
    pub fn max_age(&self) -> u32 {
        match self {
            DataSourceTarget::WithMaxAge(max_age) => *max_age,
            DataSourceTarget::ForceCache => u32::MAX,
            DataSourceTarget::ForceDevice => 0,
        }
    }
}

impl TryFromNative<crate::bindings::da::tagOPCDATASOURCE> for DataSourceTarget {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCDATASOURCE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_DS_CACHE => Ok(DataSourceTarget::ForceCache),
            crate::bindings::da::OPC_DS_DEVICE => Ok(DataSourceTarget::ForceDevice),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown data source: {unknown:?}"),
            )),
        }
    }
}

impl TryToNative<crate::bindings::da::tagOPCDATASOURCE> for DataSourceTarget {
    fn try_to_native(&self) -> windows::core::Result<crate::bindings::da::tagOPCDATASOURCE> {
        match self {
            DataSourceTarget::ForceCache => Ok(crate::bindings::da::OPC_DS_CACHE),
            DataSourceTarget::ForceDevice => Ok(crate::bindings::da::OPC_DS_DEVICE),
            DataSourceTarget::WithMaxAge(_) => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "MaxAge data source requires a value",
            )),
        }
    }
}

/// Full read value result carrying value, quality, and timestamp.
pub struct ItemValue {
    pub value: windows::Win32::System::Variant::VARIANT,
    pub quality: u16,
    pub timestamp: std::time::SystemTime,
}

impl
    TryFromNative<(
        RemoteArray<windows::Win32::System::Variant::VARIANT>,
        RemoteArray<u16>,
        RemoteArray<windows::Win32::Foundation::FILETIME>,
        RemoteArray<windows::core::HRESULT>,
    )> for Vec<windows::core::Result<ItemValue>>
{
    fn try_from_native(
        native: &(
            RemoteArray<windows::Win32::System::Variant::VARIANT>,
            RemoteArray<u16>,
            RemoteArray<windows::Win32::Foundation::FILETIME>,
            RemoteArray<windows::core::HRESULT>,
        ),
    ) -> windows::core::Result<Self> {
        let (values, qualities, timestamps, errors) = native;

        if values.len() != qualities.len()
            || values.len() != timestamps.len()
            || values.len() != errors.len()
        {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "Arrays have different lengths",
            ));
        }

        Ok(values
            .as_slice()
            .iter()
            .zip(qualities.as_slice())
            .zip(timestamps.as_slice())
            .zip(errors.as_slice())
            .map(|(((value, quality), timestamp), error)| {
                if error.is_ok() {
                    Ok(ItemValue {
                        value: value.clone(),
                        quality: *quality,
                        timestamp: try_from_native!(timestamp),
                    })
                } else {
                    Err((*error).into())
                }
            })
            .collect())
    }
}

/// Item value struct for writes or partial updates.
pub struct ItemPartialValue {
    pub value: windows::Win32::System::Variant::VARIANT,
    pub quality: Option<u16>,
    pub timestamp: Option<std::time::SystemTime>,
}

// try to native
impl TryToNative<crate::bindings::da::tagOPCITEMVQT> for ItemPartialValue {
    fn try_to_native(&self) -> windows::core::Result<crate::bindings::da::tagOPCITEMVQT> {
        Ok(crate::bindings::da::tagOPCITEMVQT {
            vDataValue: self.value.clone(),
            bQualitySpecified: self.quality.is_some().into(),
            wQuality: self.quality.unwrap_or_default(),
            bTimeStampSpecified: self.timestamp.is_some().into(),
            ftTimeStamp: self
                .timestamp
                .map(|t| t.try_to_native())
                .transpose()?
                .unwrap_or_default(),
            wReserved: 0,
            dwReserved: 0,
        })
    }
}

/// Filter type for navigating the namespace (Branch, Leaf, Flat).
pub enum BrowseType {
    Branch,
    Leaf,
    Flat,
}

impl TryFromNative<crate::bindings::da::tagOPCBROWSETYPE> for BrowseType {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCBROWSETYPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_BRANCH => Ok(BrowseType::Branch),
            crate::bindings::da::OPC_LEAF => Ok(BrowseType::Leaf),
            crate::bindings::da::OPC_FLAT => Ok(BrowseType::Flat),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown browse type: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCBROWSETYPE> for BrowseType {
    fn to_native(&self) -> crate::bindings::da::tagOPCBROWSETYPE {
        match self {
            BrowseType::Branch => crate::bindings::da::OPC_BRANCH,
            BrowseType::Leaf => crate::bindings::da::OPC_LEAF,
            BrowseType::Flat => crate::bindings::da::OPC_FLAT,
        }
    }
}

/// Granular filter for enumeration results.
pub enum BrowseFilter {
    All,
    Branches,
    Items,
}

impl TryFromNative<crate::bindings::da::tagOPCBROWSEFILTER> for BrowseFilter {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCBROWSEFILTER,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_BROWSE_FILTER_ALL => Ok(BrowseFilter::All),
            crate::bindings::da::OPC_BROWSE_FILTER_BRANCHES => Ok(BrowseFilter::Branches),
            crate::bindings::da::OPC_BROWSE_FILTER_ITEMS => Ok(BrowseFilter::Items),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown browse filter: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCBROWSEFILTER> for BrowseFilter {
    fn to_native(&self) -> crate::bindings::da::tagOPCBROWSEFILTER {
        match self {
            BrowseFilter::All => crate::bindings::da::OPC_BROWSE_FILTER_ALL,
            BrowseFilter::Branches => crate::bindings::da::OPC_BROWSE_FILTER_BRANCHES,
            BrowseFilter::Items => crate::bindings::da::OPC_BROWSE_FILTER_ITEMS,
        }
    }
}

/// Typology of the server's address space.
pub enum NamespaceType {
    Flat,
    Hierarchy,
}

impl TryFromNative<crate::bindings::da::tagOPCNAMESPACETYPE> for NamespaceType {
    fn try_from_native(
        native: &crate::bindings::da::tagOPCNAMESPACETYPE,
    ) -> windows::core::Result<Self> {
        match *native {
            crate::bindings::da::OPC_NS_HIERARCHIAL => Ok(NamespaceType::Hierarchy),
            crate::bindings::da::OPC_NS_FLAT => Ok(NamespaceType::Flat),
            unknown => Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("Unknown namespace type: {unknown:?}"),
            )),
        }
    }
}

impl ToNative<crate::bindings::da::tagOPCNAMESPACETYPE> for NamespaceType {
    fn to_native(&self) -> crate::bindings::da::tagOPCNAMESPACETYPE {
        match self {
            NamespaceType::Hierarchy => crate::bindings::da::OPC_NS_HIERARCHIAL,
            NamespaceType::Flat => crate::bindings::da::OPC_NS_FLAT,
        }
    }
}

// COSERVERINFO
/// Information defining how to connect to a remote server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub auth_info: AuthInfo,
}

/// FFI-safe bridge for `ServerInfo` (COSERVERINFO).
pub struct ServerInfoBridge {
    pub name: LocalPointer<Vec<u16>>,
    pub auth_info: AuthInfoBridge,
}

impl IntoBridge<ServerInfoBridge> for ServerInfo {
    fn into_bridge(self) -> ServerInfoBridge {
        ServerInfoBridge {
            name: LocalPointer::from(&self.name),
            auth_info: self.auth_info.into_bridge(),
        }
    }
}

impl TryToNative<windows::Win32::System::Com::COSERVERINFO> for ServerInfoBridge {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::System::Com::COSERVERINFO> {
        Ok(windows::Win32::System::Com::COSERVERINFO {
            dwReserved1: 0,
            dwReserved2: 0,
            pwszName: self.name.as_pwstr(),
            // 指向 self.auth_info.native（Box<COAUTHINFO> 堆地址）。self（ServerInfoBridge）在
            // 调用方存活期间不 move，故该指针在返回的 COSERVERINFO 使用期内有效。
            pAuthInfo: &*self.auth_info.native as *const _ as *mut _,
        })
    }
}

/// Authentication and authorization settings for DCOM.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthInfo {
    pub authn_svc: u32,
    pub authz_svc: u32,
    pub server_principal_name: String,
    pub authn_level: u32,
    pub impersonation_level: u32,
    pub auth_identity_data: AuthIdentity,
    pub capabilities: u32,
}

impl AuthInfo {
    /// DCOM 默认认证配置：WinNT 认证服务、连接级认证、识别模拟、当前用户凭据。
    ///
    /// 适合同域/已登录用户的远程访问。跨域或特定凭据场景请构造自定义 [`AuthIdentity`]。
    /// 常量值对应 `RPC_C_AUTHN_WINNT`(10)、`RPC_C_AUTHZ_NONE`(0)、
    /// `RPC_C_AUTHN_LEVEL_CONNECT`(2)、`RPC_C_IMP_LEVEL_IDENTIFY`(2)。
    #[must_use]
    pub fn default_dcom() -> Self {
        Self {
            authn_svc: 10,
            authz_svc: 0,
            server_principal_name: String::new(),
            authn_level: 2,
            impersonation_level: 2,
            auth_identity_data: AuthIdentity {
                user: String::new(),
                domain: String::new(),
                password: String::new(),
                flags: 0,
            },
            capabilities: 0,
        }
    }
}

/// FFI-safe bridge for `AuthInfo` (COAUTHINFO).
///
/// `native` 是 owned `COAUTHINFO`：`pwszServerPrincName` 指向本结构体的 `server_principal_name`，
/// `pAuthIdentityData` 指向 `auth_identity_data.native`（`Box<COAUTHIDENTITY>` 堆地址）。
/// 用 `Box` 固定 `native` 地址，使 `ServerInfoBridge::try_to_native` 取走的 `pAuthInfo` 指针
/// 在 Bridge 存活期内有效（不再悬垂）。
pub struct AuthInfoBridge {
    pub authn_svc: u32,
    pub authz_svc: u32,
    pub server_principal_name: LocalPointer<Vec<u16>>,
    pub authn_level: u32,
    pub impersonation_level: u32,
    pub auth_identity_data: AuthIdentityBridge,
    pub capabilities: u32,
    pub native: Box<windows::Win32::System::Com::COAUTHINFO>,
}

impl IntoBridge<AuthInfoBridge> for AuthInfo {
    fn into_bridge(self) -> AuthInfoBridge {
        let princ_empty = self.server_principal_name.is_empty();
        let server_principal_name = LocalPointer::from(&self.server_principal_name);
        let auth_identity_data = self.auth_identity_data.into_bridge();
        // 指针有效性：`pAuthIdentityData` 指向 `auth_identity_data.native`（Box 堆地址，固定）；
        // `pwszServerPrincName` 指向 `server_principal_name` 的 Vec 堆缓冲。两者随后 move 进 struct，
        // 堆地址均不变，故指针在 Bridge 存活期内有效。
        let native = Box::new(windows::Win32::System::Com::COAUTHINFO {
            dwAuthnSvc: self.authn_svc,
            dwAuthzSvc: self.authz_svc,
            // MSDN: 用 RPC_C_AUTHN_WINNT 时 pwszServerPrincName 必须为 NULL；空串→null。
            pwszServerPrincName: if princ_empty {
                windows::core::PWSTR::null()
            } else {
                server_principal_name.as_pwstr()
            },
            dwAuthnLevel: self.authn_level,
            dwImpersonationLevel: self.impersonation_level,
            pAuthIdentityData: &*auth_identity_data.native as *const _ as *mut _,
            dwCapabilities: self.capabilities,
        });
        AuthInfoBridge {
            authn_svc: self.authn_svc,
            authz_svc: self.authz_svc,
            server_principal_name,
            authn_level: self.authn_level,
            impersonation_level: self.impersonation_level,
            auth_identity_data,
            capabilities: self.capabilities,
            native,
        }
    }
}

/// DCOM authentication credentials.
#[derive(Clone, PartialEq)]
pub struct AuthIdentity {
    pub user: String,
    pub domain: String,
    pub password: String,
    pub flags: u32,
}

impl std::fmt::Debug for AuthIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 屏蔽 password（与 AuthCredentials 一致）。AuthInfo/ServerInfo 的 derive(Debug)
        // 经此实现访问 auth_identity_data，故整条 Debug 链都不会泄露 password。
        f.debug_struct("AuthIdentity")
            .field("user", &self.user)
            .field("domain", &self.domain)
            .field("password", &"***")
            .field("flags", &self.flags)
            .finish()
    }
}

/// 用户态 DCOM 远程认证凭据，对应 `COAUTHIDENTITY`（见 [`AuthIdentity`]）。
///
/// 通过 [`crate::backend::connector::ComConnector::with_credentials`] 或
/// `OpcDaClient::with_credentials` 注入，用于跨域或指定账户访问远程 OPC DA Server。
/// `user` 为空时退化为当前登录用户（DCOM 默认认证）。
///
/// `Debug` 实现屏蔽 `password`，避免凭据进入日志或错误信息。
pub struct AuthCredentials {
    /// 登录用户名（空 → 用当前登录用户）。
    pub user: String,
    /// 密码明文（仅用于构造 `COAUTHIDENTITY`，不进入日志）。
    pub password: String,
    /// 域或机器名（可空；本地账户用机器名或留空）。
    pub domain: String,
}

impl AuthCredentials {
    /// 构造对应 [`AuthInfo`]：`RPC_C_AUTHN_WINNT` + `RPC_C_AUTHN_LEVEL_CONNECT` +
    /// `RPC_C_IMP_LEVEL_IMPERSONATE`（允许 server 以本 client 身份访问本地资源，如读取标签）。
    ///
    /// 常量值：`authn_svc=10`(WINNT)、`authz_svc=0`、`authn_level=2`(CONNECT)、
    /// `impersonation_level=3`(IMPERSONATE)。
    #[must_use]
    pub fn to_auth_info(&self) -> AuthInfo {
        AuthInfo {
            authn_svc: 10,
            authz_svc: 0,
            server_principal_name: String::new(),
            authn_level: 2,
            impersonation_level: 3,
            auth_identity_data: AuthIdentity {
                user: self.user.clone(),
                domain: self.domain.clone(),
                password: self.password.clone(),
                // SEC_WINNT_AUTH_IDENTITY_UNICODE(2)：User/Domain/Password 是 UTF-16 wide string。
                // （0 无效，会让 DCOM 按错误编码解析凭据 → 0x80070057 E_INVALIDARG。）
                flags: 2,
            },
            capabilities: 0,
        }
    }
}

impl Clone for AuthCredentials {
    fn clone(&self) -> Self {
        Self {
            user: self.user.clone(),
            password: self.password.clone(),
            domain: self.domain.clone(),
        }
    }
}

impl std::fmt::Debug for AuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 屏蔽 password，避免凭据随 Debug 进入日志/错误。
        f.debug_struct("AuthCredentials")
            .field("user", &self.user)
            .field("password", &"***")
            .field("domain", &self.domain)
            .finish()
    }
}

/// 宽字符串的字符数（不含 null 终止符），用于 `COAUTHIDENTITY` 的 `*Length` 字段。
///
/// `LocalPointer::from` 生成的 wide string 带 null 终止符，故 `len() - 1` 得字符数。
/// 用户名/域名/密码长度不可能接近 u32 上限，截断无实际风险。
#[allow(clippy::cast_possible_truncation)]
fn wide_char_len(s: &LocalPointer<Vec<u16>>) -> u32 {
    (s.len().saturating_sub(1)) as u32
}

/// FFI-safe bridge for `AuthIdentity` (COAUTHIDENTITY).
///
/// `native` 是 owned 的 `COAUTHIDENTITY`，其 `User`/`Domain`/`Password` 指针指向本
/// 结构体里 `user`/`domain`/`password` 的 wide string 堆缓冲。整条数据链
/// （`Box<COAUTHIDENTITY>` 与三个 `LocalPointer<Vec<u16>>`）都由 Bridge owned 且都在堆上，
/// Bridge 整体 move 后这些指针仍指向有效内存——不再有旧实现返回临时值导致的悬垂指针
/// （曾致 32 位 `0x800703E6`）。
pub struct AuthIdentityBridge {
    pub user: LocalPointer<Vec<u16>>,
    pub domain: LocalPointer<Vec<u16>>,
    pub password: LocalPointer<Vec<u16>>,
    pub flags: u32,
    pub native: Box<windows::Win32::System::Com::COAUTHIDENTITY>,
}

impl IntoBridge<AuthIdentityBridge> for AuthIdentity {
    fn into_bridge(self) -> AuthIdentityBridge {
        let user = LocalPointer::from(&self.user);
        let domain = LocalPointer::from(&self.domain);
        let password = LocalPointer::from(&self.password);
        // 指针有效性：`native` 的裸指针指向上面三个 `LocalPointer` 的 `Vec<u16>` 堆缓冲。
        // 它们随后 move 进 struct，但 `Vec` 堆地址不变，故指针在 Bridge 存活期内始终有效。
        let native = Box::new(windows::Win32::System::Com::COAUTHIDENTITY {
            User: user.as_pwstr().0,
            UserLength: wide_char_len(&user),
            Domain: domain.as_pwstr().0,
            DomainLength: wide_char_len(&domain),
            Password: password.as_pwstr().0,
            PasswordLength: wide_char_len(&password),
            Flags: self.flags,
        });
        AuthIdentityBridge {
            user,
            domain,
            password,
            flags: self.flags,
            native,
        }
    }
}

/// COM instantiation context flags (CLSCTX).
#[derive(Debug, Clone, PartialEq)]
pub enum ClassContext {
    All,
    InProcServer,
    InProcHandler,
    LocalServer,
    InProcServer16,
    RemoteServer,
    InProcHandler16,
    NoCodeDownload,
    NoCustomMarshal,
    EnableCodeDownload,
    NoFailureLog,
    DisableAAA,
    EnableAAA,
    FromDefaultContext,
    ActivateX86Server,
    Activate32BitServer,
    Activate64BitServer,
    EnableCloaking,
    AppContainer,
    ActivateAAAAsIU,
    ActivateARM32Server,
    AllowLowerTrustRegistration,
    PsDll,
}

impl ToNative<windows::Win32::System::Com::CLSCTX> for ClassContext {
    fn to_native(&self) -> windows::Win32::System::Com::CLSCTX {
        match self {
            ClassContext::All => windows::Win32::System::Com::CLSCTX_ALL,
            ClassContext::InProcServer => windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            ClassContext::InProcHandler => windows::Win32::System::Com::CLSCTX_INPROC_HANDLER,
            ClassContext::LocalServer => windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
            ClassContext::InProcServer16 => windows::Win32::System::Com::CLSCTX_INPROC_SERVER16,
            ClassContext::RemoteServer => windows::Win32::System::Com::CLSCTX_REMOTE_SERVER,
            ClassContext::InProcHandler16 => windows::Win32::System::Com::CLSCTX_INPROC_HANDLER16,
            ClassContext::NoCodeDownload => windows::Win32::System::Com::CLSCTX_NO_CODE_DOWNLOAD,
            ClassContext::NoCustomMarshal => windows::Win32::System::Com::CLSCTX_NO_CUSTOM_MARSHAL,
            ClassContext::EnableCodeDownload => {
                windows::Win32::System::Com::CLSCTX_ENABLE_CODE_DOWNLOAD
            }
            ClassContext::NoFailureLog => windows::Win32::System::Com::CLSCTX_NO_FAILURE_LOG,
            ClassContext::DisableAAA => windows::Win32::System::Com::CLSCTX_DISABLE_AAA,
            ClassContext::EnableAAA => windows::Win32::System::Com::CLSCTX_ENABLE_AAA,
            ClassContext::FromDefaultContext => {
                windows::Win32::System::Com::CLSCTX_FROM_DEFAULT_CONTEXT
            }
            ClassContext::ActivateX86Server => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_X86_SERVER
            }
            ClassContext::Activate32BitServer => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_32_BIT_SERVER
            }
            ClassContext::Activate64BitServer => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_64_BIT_SERVER
            }
            ClassContext::EnableCloaking => windows::Win32::System::Com::CLSCTX_ENABLE_CLOAKING,
            ClassContext::AppContainer => windows::Win32::System::Com::CLSCTX_APPCONTAINER,
            ClassContext::ActivateAAAAsIU => windows::Win32::System::Com::CLSCTX_ACTIVATE_AAA_AS_IU,
            ClassContext::ActivateARM32Server => {
                windows::Win32::System::Com::CLSCTX_ACTIVATE_ARM32_SERVER
            }
            ClassContext::AllowLowerTrustRegistration => {
                windows::Win32::System::Com::CLSCTX_ALLOW_LOWER_TRUST_REGISTRATION
            }
            ClassContext::PsDll => windows::Win32::System::Com::CLSCTX_PS_DLL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opc_da::com_utils::{IntoBridge as _, TryToNative as _};

    /// 读取 null 结尾 wide string 为 `String`（仅测试用）。
    fn read_wide(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: `ptr` 来自 `LocalPointer` 内 null 结尾的 wide string，向后扫描到 0 安全。
        unsafe {
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        }
    }

    /// P1.1 回归：`AuthIdentityBridge.native` 的指针在 Bridge move 后仍有效
    /// （旧实现 `try_to_native` 返回临时 → 悬垂；新实现 `native` 是 owned `Box`）。
    #[test]
    fn auth_identity_native_pointers_survive_move() {
        let id = AuthIdentity {
            user: "viccom".to_string(),
            domain: "PLANT".to_string(),
            password: "test-password".to_string(),
            flags: 0,
        };
        let bridge = id.into_bridge();
        let moved = bridge; // 模拟调用栈中的 move
        let native = &*moved.native;
        assert_eq!(native.UserLength, 6);
        assert_eq!(native.DomainLength, 5);
        assert_eq!(native.PasswordLength, 13);
        assert_eq!(read_wide(native.User), "viccom");
        assert_eq!(read_wide(native.Domain), "PLANT");
        assert_eq!(read_wide(native.Password), "test-password");
    }

    /// P1.1 回归：三层 Bridge（ServerInfo→AuthInfo→AuthIdentity）move 后 `pAuthInfo` 链仍有效。
    #[test]
    fn server_info_bridge_auth_chain_survives_move() {
        let info = ServerInfo {
            name: "192.168.199.155".to_string(),
            auth_info: AuthInfo {
                authn_svc: 10,
                authz_svc: 0,
                server_principal_name: String::new(),
                authn_level: 2,
                impersonation_level: 2,
                auth_identity_data: AuthIdentity {
                    user: "viccom".to_string(),
                    domain: String::new(),
                    password: "test-password".to_string(),
                    flags: 0,
                },
                capabilities: 0,
            },
        };
        let bridge = info.into_bridge();
        let moved = bridge;
        let server = moved
            .try_to_native()
            .expect("ServerInfoBridge::try_to_native ok");
        assert!(!server.pAuthInfo.is_null(), "pAuthInfo 必须非空");
        // SAFETY: `pAuthInfo` 指向 `moved.auth_info.native`（`Box<COAUTHINFO>` 堆地址，moved 存活）。
        let auth = unsafe { &*server.pAuthInfo };
        assert!(
            !auth.pAuthIdentityData.is_null(),
            "pAuthIdentityData 必须非空"
        );
        // SAFETY: `pAuthIdentityData` 指向 `moved.auth_info.auth_identity_data.native`（`Box` 堆）。
        let ident = unsafe { &*auth.pAuthIdentityData };
        assert_eq!(read_wide(ident.User), "viccom");
        assert_eq!(read_wide(ident.Password), "test-password");
    }
}
