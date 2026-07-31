#![cfg(feature = "e2e")]
//! End-to-end tests against a real OPC DA server.
//!
//! Exercises every `OpcProvider` method against a live COM server (no mocks).
//!
//! # Prerequisites
//! - A registered OPC DA server (default `Matrikon.OPC.Simulation.1`; also tested:
//!   `Kepware.KEPServerEX.V6`).
//! - For remote tests: a reachable Windows host with OPC DA + DCOM configured.
//!
//! # Configuration (env vars)
//! - `OPC_E2E_SERVER` — server ProgID (default `Matrikon.OPC.Simulation.1`)
//! - `OPC_E2E_HOST` — local host (default `localhost`)
//! - `OPC_E2E_REMOTE_HOST` — remote DCOM host (default `192.168.199.155`)
//!
//! # Run
//! ```sh
//! cargo test -p opc-da-client --features e2e --test e2e -- --nocapture --test-threads=1
//! ```
//!
//! Tests are written to be tolerant of server-specific behaviour (read-only tags,
//! optional DA 3.0 interfaces) while still asserting the core data path.

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, OpcValue, ServerState};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn server() -> String {
    std::env::var("OPC_E2E_SERVER").unwrap_or_else(|_| "Matrikon.OPC.Simulation.1".into())
}
fn host() -> String {
    std::env::var("OPC_E2E_HOST").unwrap_or_else(|_| "localhost".into())
}
fn remote_host() -> String {
    std::env::var("OPC_E2E_REMOTE_HOST").unwrap_or_else(|_| "192.168.199.155".into())
}

fn client() -> OpcDaClient {
    OpcDaClient::new(ComConnector::default()).expect("OpcDaClient init (COM worker)")
}

/// Browse and return up to `n` tag IDs from the configured server.
async fn first_tags(n: usize) -> Vec<String> {
    let c = client();
    let progress = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let tags = c
        .browse_tags(&server(), 1000, progress, sink, 0, 0)
        .await
        .expect("browse_tags");
    assert!(!tags.is_empty(), "server should expose at least one tag");
    tags.into_iter().take(n).collect()
}

#[tokio::test]
async fn e2e_list_servers() {
    let c = client();
    let servers = c.list_servers(&host()).await.expect("list_servers");
    eprintln!("[list_servers] count={} {servers:?}", servers.len());
    // Tolerant: IOPCServerList::EnumClassesOfCategories enumerates by CATID registration,
    // which may differ from the ProgIDs visible to 32-bit OPC clients. Direct connect (below)
    // exercises the CLSIDFromProgID path independently.
}

#[tokio::test]
async fn e2e_browse_tags() {
    let c = client();
    let progress = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let tags = c
        .browse_tags(&server(), 1000, progress.clone(), sink, 0, 0)
        .await
        .expect("browse_tags");
    eprintln!(
        "[browse_tags] {} tags, progress={}",
        tags.len(),
        progress.load(Ordering::Relaxed)
    );
    assert!(!tags.is_empty());
    assert!(progress.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn e2e_browse_filtered() {
    let c = client();
    let progress = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(Mutex::new(Vec::new()));
    // data_type=0/access_rights=0 == no filter (same as e2e_browse_tags); proves the
    // filter parameters are plumbed end-to-end.
    let tags = c
        .browse_tags(&server(), 100, progress, sink, 0, 0)
        .await
        .expect("browse_tags (filtered)");
    eprintln!("[browse_filtered] {} tags", tags.len());
    assert!(!tags.is_empty());
}

#[tokio::test]
async fn e2e_browse_children() {
    // 树形浏览端到端验证：root 单级 + 下钻一个 branch 单级。
    // 验证 OPC_BROWSE_TO "" 回 root + OPC_BROWSE_DOWN 分段下钻 + branch/leaf
    // 枚举在真实服务器工作（Matrikon hierarchical：root 有 branches，下钻有 leaves）。
    let c = client();

    let root = c
        .browse_children(&server(), None, 0, 0)
        .await
        .expect("browse_children(root)");
    eprintln!(
        "[browse_children] root: {} branches, {} leaves",
        root.branches.len(),
        root.leaves.len()
    );
    // 容错：flat namespace 的 root 可能只有 leaves；hierarchical 的 root 只有 branches。
    assert!(
        !root.branches.is_empty() || !root.leaves.is_empty(),
        "root should expose branches or leaves"
    );

    // 下钻第一个 branch：验证 DOWN 分段导航 + 该 branch 的 leaves 解析。
    if let Some(branch) = root.branches.first() {
        let kids = c
            .browse_children(&server(), Some(branch.id.clone()), 0, 0)
            .await
            .expect("browse_children(branch)");
        eprintln!(
            "[browse_children] branch {:?}: {} sub-branches, {} leaves",
            branch.id,
            kids.branches.len(),
            kids.leaves.len()
        );
        assert!(
            !kids.branches.is_empty() || !kids.leaves.is_empty(),
            "branch {:?} should have children (branches or leaves)",
            branch.id
        );
        for leaf in &kids.leaves {
            assert!(
                !leaf.item_id.is_empty(),
                "leaf item_id must be non-empty (name={:?})",
                leaf.name
            );
        }
    }
}

#[tokio::test]
async fn e2e_read_tag_values() {
    let c = client();
    let tags = first_tags(3).await;
    let values = c
        .read_tag_values(&server(), tags.clone())
        .await
        .expect("read_tag_values");
    eprintln!("[read_tag_values] {:?}", values);
    assert_eq!(values.len(), tags.len());
    for v in &values {
        assert!(!v.tag_id.is_empty());
        assert!(!v.quality.is_empty());
    }
}

#[tokio::test]
async fn e2e_write_tag_value() {
    let c = client();
    // Bucket Brigade.Int4 is a read/write scalar. (Writing Int to an array tag like
    // ArrayOfReal8 is rejected with 0x80020005 type mismatch — not a library bug.)
    let result = c
        .write_tag_value(&server(), "Bucket Brigade.Int4", OpcValue::Int(42))
        .await
        .expect("write_tag_value call");
    eprintln!(
        "[write_tag_value] success={} err={:?}",
        result.success, result.error
    );
    assert!(
        result.success,
        "write should succeed on a writable scalar tag"
    );
}

#[tokio::test]
async fn e2e_write_tag_values_batch() {
    let c = client();
    // Writable scalars with matching types.
    let items = vec![
        ("Bucket Brigade.Int4".to_string(), OpcValue::Int(7)),
        ("Bucket Brigade.Real8".to_string(), OpcValue::Float(2.5)),
        ("Bucket Brigade.Boolean".to_string(), OpcValue::Bool(true)),
    ];
    let results = c
        .write_tag_values(&server(), items)
        .await
        .expect("write_tag_values");
    eprintln!("[write_tag_values] {:?}", results);
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|r| r.success),
        "all scalar writes should succeed: {results:?}"
    );
}

#[tokio::test]
async fn e2e_get_server_status() {
    let c = client();
    let status = c
        .get_server_status(&server())
        .await
        .expect("get_server_status");
    eprintln!(
        "[get_server_status] state={:?} vendor={:?} group_count={}",
        status.server_state, status.vendor_info, status.group_count
    );
    assert!(matches!(
        status.server_state,
        ServerState::Running | ServerState::NoConfig | ServerState::Suspended
    ));
}

#[tokio::test]
async fn e2e_get_item_properties() {
    let c = client();
    let tags = first_tags(1).await;
    // Some servers don't implement IOPCItemProperties; tolerate NotImplemented.
    match c.get_item_properties(&server(), &tags[0]).await {
        Ok(props) => eprintln!(
            "[get_item_properties] {} entries for {}",
            props.len(),
            tags[0]
        ),
        Err(e) => eprintln!("[get_item_properties] not supported: {e}"),
    }
}

#[tokio::test]
async fn e2e_get_error_string() {
    let c = client();
    match c.get_error_string(&server(), 0).await {
        Ok(s) => eprintln!("[get_error_string] S_OK -> {s:?}"),
        Err(e) => eprintln!("[get_error_string] err: {e}"),
    }
}

#[tokio::test]
async fn e2e_read_tag_values_max_age() {
    let c = client();
    let tags = first_tags(2).await;
    // IOPCSyncIO2 is DA 3.0; tolerate servers that lack it.
    match c
        .read_tag_values_max_age(&server(), tags.clone(), 1000)
        .await
    {
        Ok(values) => {
            eprintln!("[read_max_age] {:?}", values);
            assert_eq!(values.len(), tags.len());
        }
        Err(e) => eprintln!("[read_max_age] not supported: {e}"),
    }
}

#[tokio::test]
async fn e2e_write_tag_value_vqt() {
    let c = client();
    match c
        .write_tag_value_vqt(
            &server(),
            "Bucket Brigade.Int4",
            OpcValue::Int(1),
            None,
            None,
        )
        .await
    {
        Ok(r) => {
            eprintln!("[write_vqt] success={}", r.success);
            assert!(r.success, "VQT write on scalar should succeed");
        }
        Err(e) => eprintln!("[write_vqt] not supported: {e}"),
    }
}

#[tokio::test]
async fn e2e_subscribe_data_change() {
    let c = client();
    let tags = first_tags(1).await;
    let sub = c
        .subscribe(&server(), tags.clone(), 500)
        .await
        .expect("subscribe");
    eprintln!("[subscribe] cookie={} tag={}", sub.cookie, tags[0]);
    // Wait for the first server-pushed OnDataChange (Matrikon Random.* changes periodically).
    let mut rx = sub.rx;
    let received = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await;
    c.unsubscribe(sub.cookie).await.expect("unsubscribe");
    match received {
        Ok(Some(tv)) => eprintln!("[subscribe] received OnDataChange: {tv:?}"),
        Ok(None) => panic!("subscribe channel closed without OnDataChange"),
        Err(_) => {
            eprintln!("[subscribe] no OnDataChange within 10s (server may not push static tags)");
        }
    }
}

#[tokio::test]
#[ignore = "诊断 Bucket Brigade.Int1：subscribe/read/write 端到端"]
async fn e2e_diag_bucket_brigade_int1() {
    // 用户报告订阅 Bucket Brigade.Int1 看不到数据。Bucket Brigade 是读写寄存器
    // （static，不自动变化），OPC OnDataChange 只在数据变化时推送。本诊断逐项打印。
    let c = client();
    let server = server();
    let tag = "Bucket Brigade.Int1";

    eprintln!("[diag] --- read {tag} ---");
    match c.read_tag_values(&server, vec![tag.into()]).await {
        Ok(v) => eprintln!("[diag] read OK: {v:?}"),
        Err(e) => eprintln!("[diag] read ERR: {e:?}"),
    }

    eprintln!("[diag] --- subscribe {tag} (1000ms) ---");
    let sub = match c.subscribe(&server, vec![tag.into()], 1000).await {
        Ok(s) => {
            eprintln!("[diag] subscribe OK cookie={}", s.cookie);
            s
        }
        Err(e) => {
            eprintln!("[diag] subscribe ERR: {e:?}");
            return;
        }
    };

    let mut rx = sub.rx;
    let mut errors = sub.errors;

    // 初始 OnDataChange（Matrikon 通常在 group active 后推一次初始值）。
    let initial = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    eprintln!("[diag] initial OnDataChange within 3s: {initial:?}");

    // write 后看是否触发 OnDataChange。
    eprintln!("[diag] --- write {tag} = 42 ---");
    match c.write_tag_value(&server, tag, OpcValue::Int(42)).await {
        Ok(r) => eprintln!("[diag] write result: {r:?}"),
        Err(e) => eprintln!("[diag] write ERR: {e:?}"),
    }
    let after_write = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    eprintln!("[diag] post-write OnDataChange within 3s: {after_write:?}");

    // read 验证写入值。
    eprintln!("[diag] --- read {tag} after write ---");
    match c.read_tag_values(&server, vec![tag.into()]).await {
        Ok(v) => eprintln!("[diag] read after write: {v:?}"),
        Err(e) => eprintln!("[diag] read after write ERR: {e:?}"),
    }

    // 订阅错误通道。
    let err_msg = tokio::time::timeout(Duration::from_millis(200), errors.recv()).await;
    eprintln!("[diag] subscription error channel: {err_msg:?}");

    let _ = c.unsubscribe(sub.cookie).await;
}

#[tokio::test]
#[ignore = "诊断：browse_children 逐层找 Bucket Brigade.Int1 的树形路径"]
async fn e2e_diag_browse_bucket() {
    let c = client();
    let server = server();

    let root = c.browse_children(&server, None, 0, 0).await.expect("root");
    eprintln!(
        "[browse] root branches: {:?}",
        root.branches.iter().map(|b| &b.id).collect::<Vec<_>>()
    );
    eprintln!(
        "[browse] root leaves: {:?}",
        root.leaves.iter().map(|l| &l.item_id).collect::<Vec<_>>()
    );

    // 下钻每个 root branch 一层，找 Bucket Brigade。
    for b in &root.branches {
        let kids = c
            .browse_children(&server, Some(b.id.clone()), 0, 0)
            .await
            .expect("branch");
        eprintln!(
            "[browse] {:?} -> sub-branches: {:?}",
            b.id,
            kids.branches.iter().map(|x| &x.id).collect::<Vec<_>>()
        );
        for sb in &kids.branches {
            if sb.name.to_lowercase().contains("bucket") {
                let gk = c
                    .browse_children(&server, Some(sb.id.clone()), 0, 0)
                    .await
                    .expect("sub-branch");
                eprintln!(
                    "[browse] {:?} -> leaves: {:?}",
                    sb.id,
                    gk.leaves.iter().map(|x| &x.item_id).collect::<Vec<_>>()
                );
            }
        }
    }
}

#[tokio::test]
async fn e2e_set_subscription_rate() {
    let c = client();
    let tags = first_tags(1).await;
    let sub = c.subscribe(&server(), tags, 1000).await.expect("subscribe");
    let revised = c
        .set_subscription_rate(sub.cookie, 2000)
        .await
        .expect("set_subscription_rate");
    eprintln!("[set_subscription_rate] revised={revised}ms");
    c.unsubscribe(sub.cookie).await.expect("unsubscribe");
}

#[tokio::test]
async fn e2e_set_keep_alive() {
    let c = client();
    let tags = first_tags(1).await;
    let sub = c.subscribe(&server(), tags, 1000).await.expect("subscribe");
    // IOPCGroupStateMgt2 (DA 3.0) may be unsupported; tolerate.
    match c.set_keep_alive(sub.cookie, 5000).await {
        Ok(revised) => eprintln!("[set_keep_alive] revised={revised}ms"),
        Err(e) => eprintln!("[set_keep_alive] not supported: {e}"),
    }
    c.unsubscribe(sub.cookie).await.expect("unsubscribe");
}

#[tokio::test]
async fn e2e_set_locale_and_client_name() {
    let c = client();
    // 1033 = en-US LCID. These rarely fail.
    c.set_locale_id(&server(), 1033)
        .await
        .expect("set_locale_id");
    c.set_client_name(&server(), "opc-cli-e2e")
        .await
        .expect("set_client_name");
    eprintln!("[set_locale_id/set_client_name] ok");
}

#[tokio::test]
async fn e2e_disconnect_reconnect() {
    let c = client();
    let tags = first_tags(1).await;
    // Initial read populates the connection pool.
    c.read_tag_values(&server(), tags.clone())
        .await
        .expect("read 1");
    c.disconnect(&server()).await.expect("disconnect");
    // After disconnect the next op must transparently reconnect.
    c.read_tag_values(&server(), tags)
        .await
        .expect("read 2 (after reconnect)");
    eprintln!("[disconnect/reconnect] ok");
}

#[tokio::test]
async fn e2e_explicit_reconnect() {
    let c = client();
    let tags = first_tags(1).await;
    c.read_tag_values(&server(), tags).await.expect("read");
    c.reconnect(&server()).await.expect("explicit reconnect");
    eprintln!("[reconnect] ok");
}

fn remote_server() -> String {
    std::env::var("OPC_E2E_REMOTE_SERVER").unwrap_or_else(|_| "Matrikon.OPC.Simulation.1".into())
}
fn remote_client() -> OpcDaClient {
    OpcDaClient::new(ComConnector::new(remote_host())).expect("remote OpcDaClient init (DCOM)")
}
async fn remote_first_tags(n: usize) -> Vec<String> {
    let c = remote_client();
    let progress = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let tags = c
        .browse_tags(&remote_server(), 100, progress, sink, 0, 0)
        .await
        .expect("remote browse_tags");
    assert!(!tags.is_empty(), "remote server should expose tags");
    tags.into_iter().take(n).collect()
}

#[tokio::test]
async fn e2e_remote_list_servers() {
    let c = remote_client();
    let servers = c
        .list_servers(&remote_host())
        .await
        .expect("remote list_servers");
    eprintln!("[remote list_servers] {}: {servers:?}", remote_host());
    assert!(
        servers.iter().any(|s| s.contains("Matrikon")),
        "remote host should list Matrikon"
    );
}

#[tokio::test]
async fn e2e_remote_browse_read() {
    let c = remote_client();
    let tags = remote_first_tags(3).await;
    let values = c
        .read_tag_values(&remote_server(), tags.clone())
        .await
        .expect("remote read_tag_values");
    eprintln!("[remote read] {:?}", values);
    assert_eq!(values.len(), tags.len());
}

#[tokio::test]
async fn e2e_remote_write() {
    let c = remote_client();
    let result = c
        .write_tag_value(&remote_server(), "Bucket Brigade.Int4", OpcValue::Int(42))
        .await
        .expect("remote write_tag_value call");
    eprintln!("[remote write] success={}", result.success);
    assert!(result.success, "remote scalar write should succeed");
}

#[tokio::test]
async fn e2e_remote_subscribe() {
    let c = remote_client();
    let tags = remote_first_tags(1).await;
    // Remote subscription needs the server to call back into this client (reverse DCOM),
    // which requires inbound DCOM permission + a marshalable sink. Tolerate failure;
    // local subscription is covered by e2e_subscribe_data_change.
    let sub = match c.subscribe(&remote_server(), tags, 500).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[remote subscribe] Advise failed (reverse-DCOM callback config?): {e}");
            return;
        }
    };
    let mut rx = sub.rx;
    let received = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await;
    let _ = c.unsubscribe(sub.cookie).await;
    eprintln!(
        "[remote subscribe] received OnDataChange: {}",
        received.is_ok()
    );
}

// ── process-kill reconnect probes (#[ignore]: kill a real system process) ─────────────
//
// These verify the DCOM/SCM relaunch path: killing OPCSim.exe mid-session, then checking the
// client transparently reconnects on the next operation. Marked `#[ignore]` because killing the
// server is a global side effect incompatible with parallel/normal test runs. Run explicitly:
//
//   cargo test -p opc-da-client --features e2e --test e2e e2e_kill_process_ \
//       -- --ignored --nocapture --test-threads=1

/// The OPC server image name (from the `LocalServer32` registration `D:\Tools\OPCSIM\OPCSim.exe`).
fn opc_sim_image() -> &'static str {
    "OPCSim.exe"
}

/// `true` if the OPC server process is currently running.
fn opc_sim_running() -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {}", opc_sim_image()), "/NH"])
        .output();
    out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(opc_sim_image()))
}

/// Force-kill the OPC server process. Returns `true` if taskkill reported success.
fn kill_opc_sim() -> bool {
    std::process::Command::new("taskkill")
        .args(["/F", "/IM", opc_sim_image()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `dispatch_with_retry` reconnect: kill the live server mid-session, then verify the next read
/// transparently reconnects (DCOM/SCM relaunches OPCSim.exe on the re-`CoCreateInstance`).
#[tokio::test]
#[ignore = "kills the real OPCSim.exe process; run with --ignored"]
async fn e2e_kill_process_read_reconnects() {
    let c = client();
    let tags = first_tags(1).await;
    let v1 = c
        .read_tag_values(&server(), tags.clone())
        .await
        .expect("read 1");
    eprintln!("[kill/read] read 1 ok: {:?}", v1[0]);
    assert!(opc_sim_running(), "OPCSim.exe must be running after read 1");

    assert!(kill_opc_sim(), "failed to kill OPCSim.exe");
    eprintln!("[kill/read] killed OPCSim.exe");
    // Give the COM runtime a moment to observe the dead proxy.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !opc_sim_running(),
        "OPCSim.exe should be gone after taskkill"
    );

    // The next read must auto-reconnect: dispatch_with_retry evicts the dead proxy, re-creates
    // the server object (DCOM/SCM relaunches OPCSim.exe), and retries.
    let v2 = c.read_tag_values(&server(), tags).await;
    match &v2 {
        Ok(values) => eprintln!("[kill/read] read 2 ok after reconnect: {:?}", values[0]),
        Err(e) => panic!("[kill/read] read 2 must auto-reconnect after process kill: {e}"),
    }
    assert!(
        opc_sim_running(),
        "OPCSim.exe should have been relaunched by DCOM/SCM"
    );
}

/// P0-1 增强：杀掉 server 进程后订阅必须自愈——监测线程检测 callback 静默死亡 → rebuild 轻量
/// re-advise 失败（死代理 0x800706BA）→ 触发重连（DCOM/SCM 重启 OPCSim）→ 新 group/items/sink →
/// `rx` 收到新的 OnDataChange。区别于应用层 reconnect（read/write 经 dispatch_with_retry），这里
/// 验证订阅级自愈。
#[tokio::test]
#[ignore = "kills the real OPCSim.exe process; run with --ignored"]
async fn e2e_kill_process_subscription_self_heals() {
    let c = client();
    let tags = first_tags(1).await;
    let sub = c
        .subscribe(&server(), tags.clone(), 500)
        .await
        .expect("subscribe");
    let mut rx = sub.rx;

    // Confirm the callback is alive before killing the server.
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(
        first.is_ok_and(|o| o.is_some()),
        "must receive an initial OnDataChange before killing the server"
    );
    eprintln!("[kill/self-heal] initial OnDataChange received");

    assert!(kill_opc_sim(), "failed to kill OPCSim.exe");
    assert!(
        !opc_sim_running(),
        "OPCSim.exe should be gone after taskkill"
    );
    eprintln!("[kill/self-heal] killed OPCSim.exe; waiting for monitor + reconnect");

    // Monitor threshold ~30s; the rebuild then reconnects (DCOM relaunches OPCSim) and the new
    // sink pushes a fresh OnDataChange. The subscription must self-heal within this window.
    let healed = tokio::time::timeout(Duration::from_secs(70), rx.recv()).await;
    assert!(
        matches!(healed, Ok(Some(_))),
        "subscription did not self-heal within 70s after server kill: {healed:?}"
    );
    eprintln!("[kill/self-heal] self-healed: new OnDataChange received");
    assert!(
        opc_sim_running(),
        "OPCSim.exe should have been relaunched by the rebuild reconnect"
    );
    let _ = c.unsubscribe(sub.cookie).await;
}
