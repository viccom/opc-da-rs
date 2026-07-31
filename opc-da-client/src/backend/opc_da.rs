use crate::backend::connector::{ComConnector, ServerConnector};
use crate::com_worker::{ComRequest, ComWorker};
use crate::opc_da::errors::OpcResult;
use crate::opc_da::typedefs::ServerStatus;
use crate::provider::{
    BrowseChildren, ItemProperty, OpcProvider, OpcValue, ShutdownHandle, SubscriptionHandle,
    TagValue, WriteResult,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// Concrete [`OpcProvider`] implementation for Windows OPC DA.
///
/// Uses native `windows-rs` COM interop via the internal `opc_da` module.
pub struct OpcDaClient<C: ServerConnector + 'static = ComConnector> {
    pub worker: ComWorker<C>,
}

/// Returns the default `OpcDaClient` using native COM settings.
///
/// # Panics
///
/// Panics if the background COM worker thread cannot be started or COM
/// Multi-Threaded Apartment (MTA) initialization fails on the worker thread.
/// Use [`OpcDaClient::new`] for fallible construction.
impl Default for OpcDaClient<ComConnector> {
    fn default() -> Self {
        Self::new(ComConnector::default()).expect("Failed to initialize OpcDaClient")
    }
}

impl OpcDaClient<ComConnector> {
    /// Create an `OpcDaClient` targeting `host` with explicit DCOM credentials
    /// (`user`/`password`/`domain`), for remote OPC DA Servers the current logged-in
    /// user cannot access (cross-domain, dedicated service account, etc.).
    ///
    /// `user` 为空时退化为当前登录用户。凭据经 `COAUTHIDENTITY` 注入远程激活。
    ///
    /// # Errors
    /// Returns `Err` if the background COM worker thread cannot be started.
    pub fn with_credentials(
        host: impl Into<String>,
        credentials: crate::opc_da::typedefs::AuthCredentials,
    ) -> OpcResult<Self> {
        Self::new(ComConnector::with_credentials(host, credentials))
    }
}

impl<C: ServerConnector + 'static> OpcDaClient<C> {
    /// Creates a new `OpcDaClient` with the given connector.
    pub fn new(connector: C) -> OpcResult<Self> {
        tracing::info!("Initializing OpcDaClient...");
        let worker = ComWorker::start(Arc::new(connector))?;
        tracing::info!("OpcDaClient initialized successfully");
        Ok(Self { worker })
    }

    /// Drop any cached connection to `server`. The next operation reconnects automatically.
    ///
    /// This bypasses the `OpcProvider` data API — it is an implementation-level lifecycle
    /// hook for callers that hold a concrete [`OpcDaClient`].
    ///
    /// # Errors
    /// Currently never fails; kept as `OpcResult` for forward compatibility.
    pub async fn disconnect(&self, server: &str) -> OpcResult<()> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::Disconnect {
                server: server_owned,
                reply,
            })
            .await
    }

    /// Force a fresh connection to `server`, replacing any cached one.
    ///
    /// # Errors
    /// Returns `Err` if the (re)connection attempt fails.
    pub async fn reconnect(&self, server: &str) -> OpcResult<()> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::Reconnect {
                server: server_owned,
                reply,
            })
            .await
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl<C: ServerConnector + 'static> OpcProvider for OpcDaClient<C> {
    async fn list_servers(&self, host: &str) -> OpcResult<Vec<String>> {
        let host_owned = host.to_string();
        self.worker
            .send_request(|reply| ComRequest::ListServers {
                host: host_owned,
                reply,
            })
            .await
    }

    async fn browse_tags(
        &self,
        server: &str,
        max_tags: usize,
        progress: Arc<AtomicUsize>,
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<Vec<String>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::BrowseTags {
                server: server_owned,
                max_tags,
                progress,
                tags_sink,
                data_type,
                access_rights,
                reply,
            })
            .await
    }

    async fn browse_children(
        &self,
        server: &str,
        branch_path: Option<String>,
        data_type: u16,
        access_rights: u32,
    ) -> OpcResult<BrowseChildren> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::BrowseChildren {
                server: server_owned,
                branch_path,
                data_type,
                access_rights,
                reply,
            })
            .await
    }

    async fn read_tag_values(
        &self,
        server: &str,
        tag_ids: Vec<String>,
    ) -> OpcResult<Vec<TagValue>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::ReadTagValues {
                server: server_owned,
                tag_ids,
                reply,
            })
            .await
    }

    async fn write_tag_value(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
    ) -> OpcResult<WriteResult> {
        let server_owned = server.to_string();
        let tag_id_owned = tag_id.to_string();
        self.worker
            .send_request(|reply| ComRequest::WriteTagValue {
                server: server_owned,
                tag_id: tag_id_owned,
                value,
                reply,
            })
            .await
    }

    async fn get_server_status(&self, server: &str) -> OpcResult<ServerStatus> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::GetServerStatus {
                server: server_owned,
                reply,
            })
            .await
    }

    async fn write_tag_values(
        &self,
        server: &str,
        items: Vec<(String, OpcValue)>,
    ) -> OpcResult<Vec<WriteResult>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::WriteTagValues {
                server: server_owned,
                items,
                reply,
            })
            .await
    }

    async fn get_item_properties(
        &self,
        server: &str,
        tag_id: &str,
    ) -> OpcResult<Vec<ItemProperty>> {
        let server_owned = server.to_string();
        let tag_id_owned = tag_id.to_string();
        self.worker
            .send_request(|reply| ComRequest::GetItemProperties {
                server: server_owned,
                tag_id: tag_id_owned,
                reply,
            })
            .await
    }

    async fn read_tag_values_max_age(
        &self,
        server: &str,
        tag_ids: Vec<String>,
        max_age_ms: u32,
    ) -> OpcResult<Vec<TagValue>> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::ReadMaxAge {
                server: server_owned,
                tag_ids,
                max_age_ms,
                reply,
            })
            .await
    }

    async fn write_tag_value_vqt(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
        quality: Option<u16>,
        timestamp: Option<std::time::SystemTime>,
    ) -> OpcResult<WriteResult> {
        let server_owned = server.to_string();
        let tag_id_owned = tag_id.to_string();
        self.worker
            .send_request(|reply| ComRequest::WriteTagValueVqt {
                server: server_owned,
                tag_id: tag_id_owned,
                value,
                quality,
                timestamp,
                reply,
            })
            .await
    }

    async fn get_error_string(&self, server: &str, hresult: i32) -> OpcResult<String> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::GetErrorString {
                server: server_owned,
                hresult,
                reply,
            })
            .await
    }

    async fn subscribe(
        &self,
        server: &str,
        tag_ids: Vec<String>,
        update_rate: u32,
    ) -> OpcResult<SubscriptionHandle> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::Subscribe {
                server: server_owned,
                tag_ids,
                update_rate,
                reply,
            })
            .await
    }

    async fn unsubscribe(&self, cookie: u32) -> OpcResult<()> {
        self.worker
            .send_request(|reply| ComRequest::Unsubscribe { cookie, reply })
            .await
    }

    async fn subscribe_shutdown(&self, server: &str) -> OpcResult<ShutdownHandle> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::SubscribeShutdown {
                server: server_owned,
                reply,
            })
            .await
    }

    async fn unsubscribe_shutdown(&self, cookie: u32) -> OpcResult<()> {
        self.worker
            .send_request(|reply| ComRequest::UnsubscribeShutdown { cookie, reply })
            .await
    }

    async fn set_subscription_rate(&self, cookie: u32, update_rate: u32) -> OpcResult<u32> {
        self.worker
            .send_request(|reply| ComRequest::SetSubscriptionRate {
                cookie,
                update_rate,
                reply,
            })
            .await
    }

    async fn set_keep_alive(&self, cookie: u32, keep_alive_ms: u32) -> OpcResult<u32> {
        self.worker
            .send_request(|reply| ComRequest::SetKeepAlive {
                cookie,
                keep_alive_ms,
                reply,
            })
            .await
    }

    async fn set_locale_id(&self, server: &str, locale_id: u32) -> OpcResult<()> {
        let server_owned = server.to_string();
        self.worker
            .send_request(|reply| ComRequest::SetLocaleId {
                server: server_owned,
                locale_id,
                reply,
            })
            .await
    }

    async fn set_client_name(&self, server: &str, name: &str) -> OpcResult<()> {
        let server_owned = server.to_string();
        let name_owned = name.to_string();
        self.worker
            .send_request(|reply| ComRequest::SetClientName {
                server: server_owned,
                name: name_owned,
                reply,
            })
            .await
    }
}
