//! Server-side IPC handlers: list / pick a ProgID.
//!
//! These back the WebView's left rail (your functions 1, 2, 3).
//!
//! Note: `OpcDaClient` does not have an explicit `connect()` step — it
//! is stateless across ProgIDs and resolves the connection lazily on
//! the first operation that names a `server`. We track the active
//! `ProgID` in [`AppState::set_prog_id`] so subsequent commands can
//! pass it back to the trait's `server: &str` parameter.

use serde::{Deserialize, Serialize};
use tauri::State;

use opc_da_client::{AuthCredentials, OpcProvider, ServerStatus};

use crate::error::DesktopResult;
use crate::state::AppState;

/// One server entry returned by `list_servers`.
///
/// The `opc-da-client` 0.3.0 API returns `Vec<String>` (ProgIDs only);
/// vendor/CLSID enrichment is left for a future API extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// ProgID / programmatic identifier (the canonical key).
    pub prog_id: String,
    /// CLSID（GUID 字符串）。
    pub clsid: String,
    /// 用户类型 / 描述（厂商 + 产品名）；server 未提供则为 `None`。
    pub user_type: Option<String>,
}

/// Enumerate OPC DA servers reachable on the given host.
///
/// Independent of any "connected" state — talks directly to the OPC
/// ServerList enumerator on `host`.
#[tauri::command]
pub async fn list_servers(
    state: State<'_, AppState>,
    host: String,
) -> DesktopResult<Vec<ServerInfo>> {
    let client = state.client().await;
    let servers = client.list_servers_with_details(&host).await?;
    Ok(servers
        .into_iter()
        .map(|s| ServerInfo {
            prog_id: s.prog_id,
            clsid: s.clsid,
            user_type: s.user_type,
        })
        .collect())
}

/// Bind a ProgID. Subsequent data-plane commands operate on this server
/// until `disconnect` is called or another ProgID is bound.
#[tauri::command]
pub async fn connect(state: State<'_, AppState>, prog_id: String) -> DesktopResult<()> {
    state.set_prog_id(prog_id).await;
    Ok(())
}

/// Drop the current ProgID binding.
#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> DesktopResult<()> {
    // 先停所有订阅（纯订阅 unsubscribe + fusion readers drop），再清 ProgID。
    // 否则旧实现只清 ProgID，订阅继续刷新（既有 bug）。
    state.stop_all_subscriptions().await;
    state.clear_prog_id().await;
    Ok(())
}

/// Switch the target host, rebuilding the OPC client from scratch.
///
/// Unlike `list_servers` (which takes `host` per call), the data-plane
/// commands operate on the client bound at construction, so a host change
/// must rebuild the whole client. See [`AppState::rebuild_client`]. No-op
/// (via dedup inside `rebuild_client`) when the host is unchanged.
#[tauri::command]
pub async fn set_host(
    state: State<'_, AppState>,
    host: String,
    user: Option<String>,
    password: Option<String>,
    domain: Option<String>,
) -> DesktopResult<()> {
    // 空 user → None（用当前登录用户）；否则组装显式 DCOM 凭据。
    let creds = user.filter(|u| !u.is_empty()).map(|u| AuthCredentials {
        user: u,
        password: password.unwrap_or_default(),
        domain: domain.unwrap_or_default(),
    });
    state.rebuild_client(&host, creds).await?;
    Ok(())
}

/// `IOPCServer::GetStatus` 返回的运行时状态（连接后可查）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusDto {
    /// Server 运行状态（Running / Failed / Suspended / ...）。
    pub server_state: String,
    /// Server 启动时间（本地）。
    pub start_time: String,
    /// Server 当前时间（本地）。
    pub current_time: String,
    /// 最后数据更新时间（本地）。
    pub last_update_time: String,
    /// 当前 group 数。
    pub group_count: u32,
    /// 带宽利用率（-1 表示无限制/不支持）。
    pub band_width: i32,
    /// 版本字符串（major.minor Build build）。
    pub version: String,
    /// 厂商信息。
    pub vendor_info: String,
}

impl From<ServerStatus> for ServerStatusDto {
    #[allow(clippy::cast_possible_wrap)] // OPC band_width: u32 的 -1（无限）按约定 wrap 成 i32
    fn from(s: ServerStatus) -> Self {
        let fmt = |t: std::time::SystemTime| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y/%m/%d %H:%M:%S")
                .to_string()
        };
        Self {
            server_state: format!("{:?}", s.server_state),
            start_time: fmt(s.start_time),
            current_time: fmt(s.current_time),
            last_update_time: fmt(s.last_update_time),
            group_count: s.group_count,
            band_width: s.band_width as i32,
            version: format!(
                "{}.{} Build {}",
                s.major_version, s.minor_version, s.build_number
            ),
            vendor_info: s.vendor_info,
        }
    }
}

/// Query the connected server's runtime status (state / times / version / vendor).
#[tauri::command]
pub async fn get_server_status(state: State<'_, AppState>) -> DesktopResult<ServerStatusDto> {
    let client = state.client().await;
    let prog_id = state.prog_id().await?;
    let status = client.get_server_status(&prog_id).await?;
    Ok(ServerStatusDto::from(status))
}
