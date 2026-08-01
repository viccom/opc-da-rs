use crate::opc_da::errors::{OpcError, OpcResult};
use crate::opc_da::typedefs::ServerStatus;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

#[cfg(feature = "test-support")]
use mockall::automock;

/// A single tag's read result.
///
/// Returned by [`OpcProvider::read_tag_values`].
///
/// # Examples
///
/// ```
/// use opc_da_client::TagValue;
///
/// let tv = TagValue {
///     tag_id: "Simulation.Random.1".to_string(),
///     value: "42.5".to_string(),
///     data_type: "Float".to_string(),
///     quality: "Good".to_string(),
///     timestamp: "2026-01-01 00:00:00".to_string(),
/// };
/// assert_eq!(tv.tag_id, "Simulation.Random.1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagValue {
    /// The fully qualified tag identifier (e.g., `"Channel1.Device1.Tag1"`).
    pub tag_id: String,
    /// The current value as a display string.
    pub value: String,
    /// The value's OPC data type as a display name (e.g. `"Float"`, `"Boolean"`,
    /// `"Array of String"`), derived from the VARIANT's `vt` discriminant.
    pub data_type: String,
    /// OPC quality indicator (e.g., `"Good"`, `"Bad"`, or `"Uncertain"`).
    pub quality: String,
    /// Timestamp of the last value change, formatted as a local time string.
    pub timestamp: String,
}

/// Typed value to write to an OPC DA tag.
///
/// # Examples
///
/// ```
/// use opc_da_client::OpcValue;
///
/// let v = OpcValue::Float(3.14);
/// assert_eq!(v, OpcValue::Float(3.14));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum OpcValue {
    /// String value (`VT_BSTR`) — server may coerce to target type.
    String(String),
    /// 32-bit integer (`VT_I4`).
    Int(i32),
    /// 64-bit float (`VT_R8`).
    Float(f64),
    /// Boolean (`VT_BOOL`).
    Bool(bool),
}

/// One direct child branch of an OPC DA namespace node.
///
/// Result of [`OpcProvider::browse_children`]. A branch may itself contain
/// further branches and leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchNode {
    /// Fully-qualified branch path: parent id + separator + [`Self::name`]
    /// (root-level branches have `id == name`). The separator is
    /// server-specific (commonly `.`).
    pub id: String,
    /// Branch browse name, relative to its parent.
    pub name: String,
}

/// One direct leaf (data tag) of an OPC DA namespace node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafNode {
    /// Fully-qualified item ID, server-resolved via
    /// `IOPCBrowseServerAddressSpace::GetItemID`.
    pub item_id: String,
    /// Leaf browse name, relative to its parent.
    pub name: String,
}

/// The direct children of one namespace node — a single lazy browse level.
///
/// Drives the desktop UI's "left branch tree + right leaf list" browser:
/// the user expands a branch, the UI calls
/// [`OpcProvider::browse_children`] for that branch's path, and renders
/// `branches` as expandable nodes and `leaves` as selectable tags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowseChildren {
    /// Child branches (each may itself have children).
    pub branches: Vec<BranchNode>,
    /// Child leaves (data tags).
    pub leaves: Vec<LeafNode>,
}

/// Result of a single write operation.
///
/// # Examples
///
/// ```
/// use opc_da_client::WriteResult;
///
/// let wr = WriteResult {
///     tag_id: "Tag1".to_string(),
///     success: true,
///     error: None,
/// };
/// assert!(wr.success);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    /// The tag that was written to.
    pub tag_id: String,
    /// Whether the write succeeded.
    pub success: bool,
    /// Error message if the write failed, `None` on success.
    pub error: Option<String>,
}

/// A single property of an OPC DA item (e.g. engineering unit, data type, access rights).
///
/// Returned by [`OpcProvider::get_item_properties`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemProperty {
    /// Server-assigned property ID.
    pub id: u32,
    /// Human-readable property description (e.g. "EU", "Access Rights").
    pub description: String,
    /// Property data type as a `VT_*` value.
    pub data_type: u16,
    /// Current property value as a display string.
    pub value: String,
}

/// Handle for an active OPC DA subscription.
///
/// Produced by [`OpcProvider::subscribe`]. The caller drains `rx` for server-pushed
/// [`TagValue`]s and passes `cookie` to [`OpcProvider::unsubscribe`] to tear it down.
#[derive(Debug)]
pub struct SubscriptionHandle {
    /// Connection cookie from `IConnectionPoint::Advise`; pass to `unsubscribe`.
    pub cookie: u32,
    /// Receiver for server-pushed tag values (`rx.recv().await`).
    pub rx: tokio::sync::mpsc::Receiver<TagValue>,
    /// Receiver for subscription-level errors (P0-1 step E), e.g. a rebuild that failed
    /// after a silently-dead callback. Drain alongside `rx`; `rx` stays pure [`TagValue`].
    pub errors: tokio::sync::mpsc::Receiver<OpcError>,
}

/// Handle for a server-shutdown notification subscription.
///
/// Produced by [`OpcProvider::subscribe_shutdown`]. The receiver yields the shutdown reason
/// string when the server calls `IOPCShutdown::ShutdownRequest`.
#[derive(Debug)]
pub struct ShutdownHandle {
    /// Connection cookie from `IConnectionPoint::Advise`; pass to `unsubscribe_shutdown`.
    pub cookie: u32,
    /// Receiver for the shutdown reason string(s).
    pub rx: tokio::sync::mpsc::Receiver<String>,
}

/// 一个 OPC DA server 的富信息（[`OpcProvider::list_servers_with_details`] 返回）。
///
/// 比 [`OpcProvider::list_servers`]（只 ProgID）多 CLSID + 用户类型描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDesc {
    /// 版本相关 ProgID（连接 server 用这个，等价于 `list_servers` 返回的 String）。
    pub prog_id: String,
    /// CLSID（GUID 字符串，如 `00000000-0000-0000-0000-000000000000`）。
    pub clsid: String,
    /// 用户类型 / 描述（厂商 + 产品名）；server 未提供则为 `None`。
    pub user_type: Option<String>,
}

/// Async trait for OPC DA operations.
///
/// This is the stable public API. Backend implementations provide
/// the actual COM/DCOM interaction.
#[cfg_attr(feature = "test-support", automock)]
#[async_trait]
pub trait OpcProvider: Send + Sync {
    /// List available OPC DA servers on the given host.
    ///
    /// # Errors
    /// Returns `Err` if COM initialization fails or the server registry
    /// cannot be enumerated.
    async fn list_servers(&self, host: &str) -> OpcResult<Vec<String>>;

    /// List servers with details (CLSID + user_type description).
    ///
    /// 默认实现退化为 prog_id only（clsid 空、user_type None）——只 impl
    /// [`list_servers`](Self::list_servers) 的简单后端自动兼容。`OpcDaClient` override
    /// 调 `IOPCServerList::GetClassDetails` 填充富信息。
    ///
    /// # Errors
    /// 同 [`list_servers`](Self::list_servers)。
    async fn list_servers_with_details(&self, host: &str) -> OpcResult<Vec<ServerDesc>> {
        Ok(self
            .list_servers(host)
            .await?
            .into_iter()
            .map(|prog_id| ServerDesc {
                prog_id,
                clsid: String::new(),
                user_type: None,
            })
            .collect())
    }

    /// Browse tags recursively, pushing discoveries to `tags_sink`.
    ///
    /// `progress` is bumped (atomically) once per discovered tag and every tag
    /// id is appended to `tags_sink` as soon as it is found, so a caller that
    /// times out the `await` can still harvest partial results from either.
    /// Pass fresh `Arc::default()` values if you don't need progress
    /// observation or partial-result harvesting.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, the `ProgID` cannot be
    /// resolved, or the namespace walk encounters an unrecoverable error.
    async fn browse_tags(
        &self,
        server: &str,
        max_tags: usize,
        progress: Arc<AtomicUsize>,
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<Vec<String>>;

    /// Browse one namespace level: the direct child branches and leaves
    /// under `branch_path` (`None` or empty = root).
    ///
    /// Unlike [`browse_tags`](Self::browse_tags), this does NOT recurse —
    /// it returns only the immediate children, letting the caller expand
    /// branches lazily (one round-trip per tree-node click). Child branch
    /// ids are built as `parent + "." + name`.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails,
    /// `ChangeBrowsePosition(OPC_BROWSE_TO)` is unsupported for the given
    /// path, or the browse enumeration fails.
    async fn browse_children(
        &self,
        server: &str,
        branch_path: Option<String>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<BrowseChildren>;

    /// Read current values for the given tag IDs.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, no items can be added
    /// to the OPC group, or the synchronous read operation fails.
    async fn read_tag_values(&self, server: &str, tag_ids: Vec<String>)
    -> OpcResult<Vec<TagValue>>;

    /// Write a value to a single OPC DA tag.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, the tag cannot be added
    /// to the OPC group, or the synchronous write operation fails.
    async fn write_tag_value(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
    ) -> OpcResult<WriteResult>;

    /// Query the current status of an OPC DA server.
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or `IOPCServer::GetStatus` fails.
    async fn get_server_status(&self, server: &str) -> OpcResult<ServerStatus>;

    /// Write typed values to multiple OPC DA tags in one operation.
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or group creation fails entirely.
    /// Per-tag failures are reported as individual `WriteResult` entries, not as errors.
    async fn write_tag_values(
        &self,
        server: &str,
        items: Vec<(String, OpcValue)>,
    ) -> OpcResult<Vec<WriteResult>>;

    /// Query the available properties of an OPC DA item (EU, data type, access rights, ...).
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or `IOPCItemProperties` is unsupported.
    async fn get_item_properties(&self, server: &str, tag_id: &str)
    -> OpcResult<Vec<ItemProperty>>;

    /// Read current values with a maximum-age constraint (`IOPCSyncIO2::ReadMaxAge`).
    ///
    /// `max_age_ms` is the maximum acceptable age (ms) of a cached value before the server
    /// re-reads from the device. Applies the same `max_age_ms` to every requested tag.
    ///
    /// # Errors
    /// Returns `Err` if the connection fails, the server lacks `IOPCSyncIO2`, or no items validate.
    async fn read_tag_values_max_age(
        &self,
        server: &str,
        tag_ids: Vec<String>,
        max_age_ms: u32,
    ) -> OpcResult<Vec<TagValue>>;

    /// Write a value with optional quality and timestamp (`IOPCSyncIO2::WriteVQT`).
    ///
    /// Used for historical back-fill and test injection where the caller controls the value's
    /// quality and/or timestamp. `None` fields are left unset (server keeps its own).
    ///
    /// # Errors
    /// Returns `Err` if the connection fails, the server lacks `IOPCSyncIO2`, or the tag cannot be added.
    async fn write_tag_value_vqt(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
        quality: Option<u16>,
        timestamp: Option<std::time::SystemTime>,
    ) -> OpcResult<WriteResult>;

    /// Get a server-localized error description for an HRESULT (`IOPCCommon::GetErrorString`).
    ///
    /// `hresult` is the raw HRESULT as a signed 32-bit integer. Useful for vendor-specific
    /// codes not covered by `friendly_com_hint`.
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or `IOPCCommon` is unsupported.
    async fn get_error_string(&self, server: &str, hresult: i32) -> OpcResult<String>;

    /// Subscribe to data-change notifications for a set of tags (`IOPCDataCallback`).
    ///
    /// The server pushes updated values at roughly `update_rate`-ms intervals (or on change).
    /// The sink forwards non-blocking; drain `rx` promptly to avoid dropped updates.
    ///
    /// # Errors
    /// Returns `Err` if the connection fails, the server lacks subscription support,
    /// or items cannot be added.
    async fn subscribe(
        &self,
        server: &str,
        tag_ids: Vec<String>,
        update_rate: u32,
    ) -> OpcResult<SubscriptionHandle>;

    /// Tear down a subscription previously returned by [`Self::subscribe`].
    ///
    /// # Errors
    /// Returns `Err` if the cookie is unknown or server teardown fails.
    async fn unsubscribe(&self, cookie: u32) -> OpcResult<()>;

    /// Subscribe to server shutdown notifications (`IOPCShutdown`).
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or the server lacks shutdown notification support.
    async fn subscribe_shutdown(&self, server: &str) -> OpcResult<ShutdownHandle>;

    /// Tear down a shutdown subscription by cookie.
    ///
    /// # Errors
    /// Returns `Err` if the cookie is unknown or server teardown fails.
    async fn unsubscribe_shutdown(&self, cookie: u32) -> OpcResult<()>;

    /// Adjust the update rate of an active subscription at runtime (`IOPCGroupStateMgt::SetState`).
    ///
    /// `cookie` is from [`Self::subscribe`]. Returns the server-revised update rate in ms.
    ///
    /// # Errors
    /// Returns `Err` if the cookie is unknown or `SetState` fails.
    async fn set_subscription_rate(&self, cookie: u32, update_rate: u32) -> OpcResult<u32>;

    /// Set the keep-alive interval for an active subscription (`IOPCGroupStateMgt2::SetKeepAlive`).
    ///
    /// # Errors
    /// Returns `Err` if the cookie is unknown or `SetKeepAlive` fails.
    async fn set_keep_alive(&self, cookie: u32, keep_alive_ms: u32) -> OpcResult<u32>;

    /// Set the server locale for string localization (`IOPCCommon::SetLocaleID`).
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or `IOPCCommon` is unsupported.
    async fn set_locale_id(&self, server: &str, locale_id: u32) -> OpcResult<()>;

    /// Set the client application name (`IOPCCommon::SetClientName`).
    ///
    /// # Errors
    /// Returns `Err` if the connection fails or `IOPCCommon` is unsupported.
    async fn set_client_name(&self, server: &str, name: &str) -> OpcResult<()>;
}
