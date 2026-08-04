//! 全流程 e2e：20 flat 探针（SimDataSource server，覆盖 [`OpcProvider`] 全方法）+ hierarchical 探针（GeneratedDataSource）。
//!
//! 流程：spawn sim → 20 flat → kill → spawn generated → hierarchical → kill → 汇总。
//! 详见 `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md` §7。

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, OpcValue};

use crate::report::probe;
use crate::server_proc::{ServerChild, server_exe_path, sim_exe_path};

static PROG_ID: LazyLock<String> = LazyLock::new(|| {
    std::env::var("OPC_DA_SERVER_PROGID").unwrap_or_else(|_| "opc-da-rs.Server.1".into())
});
const HOST: &str = "localhost";

/// 20 flat 探针（连 SimDataSource server，覆盖 [`OpcProvider`] 全部方法）。`(passed, failed)`。
///
/// 1-13 核心（status/read/write/round-trip/subscribe/browse/properties/error_string/
/// list_servers/write_tag_values/locale/client_name/shutdown）；14-20 补全 OpcProvider
/// 其余方法（unsubscribe/unsubscribe_shutdown/set_subscription_rate/list_servers_with_details
/// + 预期失败的 max_age/vqt/keep_alive——server 未实装 IOPCSyncIO2/IOPCGroupStateMgt2）。
#[allow(clippy::too_many_lines)]
pub async fn run_flat() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init");
    let (mut passed, mut failed) = (0u32, 0u32);

    // 1. get_server_status（IOPCServer::GetStatus）。
    match client.get_server_status(&PROG_ID).await {
        Ok(s) => probe(
            &mut passed,
            &mut failed,
            "get_server_status",
            true,
            &format!("{s:?}"),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "get_server_status",
            false,
            &e.to_string(),
        ),
    }

    // 2. read Random.Int4（quality Good + 0..=100）。
    match client
        .read_tag_values(&PROG_ID, vec!["Random.Int4".to_string()])
        .await
    {
        Ok(vals) => {
            let ok = vals.first().is_some_and(|tv| {
                let v: i32 = tv.value.parse().unwrap_or(-1);
                tv.quality == "Good" && (0..=100).contains(&v)
            });
            probe(
                &mut passed,
                &mut failed,
                "read Random.Int4",
                ok,
                &format!("{:?}", vals.first()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "read Random.Int4",
            false,
            &e.to_string(),
        ),
    }

    // 3. write Bucket Brigade.Int4 = 42。
    match client
        .write_tag_value(&PROG_ID, "Bucket Brigade.Int4", OpcValue::Int(42))
        .await
    {
        Ok(_) => probe(
            &mut passed,
            &mut failed,
            "write Bucket Brigade=42",
            true,
            "Ok",
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "write Bucket Brigade=42",
            false,
            &e.to_string(),
        ),
    }

    // 4. read round-trip Bucket Brigade.Int4 = 42。
    match client
        .read_tag_values(&PROG_ID, vec!["Bucket Brigade.Int4".to_string()])
        .await
    {
        Ok(vals) => {
            let ok = vals.first().is_some_and(|tv| tv.value == "42");
            probe(
                &mut passed,
                &mut failed,
                "read round-trip Bucket=42",
                ok,
                &format!("{:?}", vals.first()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "read round-trip Bucket=42",
            false,
            &e.to_string(),
        ),
    }

    // 5. subscribe（3s 内收 OnDataChange）。
    match client
        .subscribe(&PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(mut handle) => {
            let r = tokio::time::timeout(Duration::from_secs(3), handle.rx.recv()).await;
            let ok = matches!(r, Ok(Some(_)));
            probe(
                &mut passed,
                &mut failed,
                "subscribe OnDataChange",
                ok,
                if ok { "收到帧" } else { "3s 未收" },
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "subscribe OnDataChange",
            false,
            &e.to_string(),
        ),
    }

    // 6. browse 4 tag（SimDataSource 全 4 leaf）。
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    match client
        .browse_tags(&PROG_ID, 100, progress, sink, 0, 0)
        .await
    {
        Ok(tags) => {
            let expected = [
                "Random.Int4",
                "Random.Real8",
                "Square Waves.Real8",
                "Bucket Brigade.Int4",
            ];
            let ok = expected.iter().all(|t| tags.contains(&(*t).to_string()));
            probe(
                &mut passed,
                &mut failed,
                "browse 4 tag",
                ok,
                &format!("{} tag", tags.len()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "browse 4 tag",
            false,
            &e.to_string(),
        ),
    }

    // 7. get_item_properties（Random.Int4）。
    match client.get_item_properties(&PROG_ID, "Random.Int4").await {
        Ok(props) => probe(
            &mut passed,
            &mut failed,
            "get_item_properties",
            !props.is_empty(),
            &format!("{} props", props.len()),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "get_item_properties",
            false,
            &e.to_string(),
        ),
    }

    // 8. get_error_string（S_OK）。
    match client.get_error_string(&PROG_ID, 0).await {
        Ok(s) => probe(
            &mut passed,
            &mut failed,
            "get_error_string",
            !s.is_empty(),
            &s,
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "get_error_string",
            false,
            &e.to_string(),
        ),
    }

    // 9. list_servers（枚举 host 上 server）。
    match client.list_servers(HOST).await {
        Ok(servers) => probe(
            &mut passed,
            &mut failed,
            "list_servers",
            true,
            &format!("{} servers: {:?}", servers.len(), servers),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "list_servers",
            false,
            &e.to_string(),
        ),
    }

    // 10. write_tag_values（多 tag write）。
    match client
        .write_tag_values(
            &PROG_ID,
            vec![("Bucket Brigade.Int4".to_string(), OpcValue::Int(99))],
        )
        .await
    {
        Ok(r) => probe(
            &mut passed,
            &mut failed,
            "write_tag_values",
            !r.is_empty(),
            &format!("{} results", r.len()),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "write_tag_values",
            false,
            &e.to_string(),
        ),
    }

    // 11. set_locale_id。
    match client.set_locale_id(&PROG_ID, 0).await {
        Ok(()) => probe(&mut passed, &mut failed, "set_locale_id", true, "Ok"),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "set_locale_id",
            false,
            &e.to_string(),
        ),
    }

    // 12. set_client_name。
    match client.set_client_name(&PROG_ID, "opc-da-client-test").await {
        Ok(()) => probe(&mut passed, &mut failed, "set_client_name", true, "Ok"),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "set_client_name",
            false,
            &e.to_string(),
        ),
    }

    // 13. subscribe_shutdown。
    match client.subscribe_shutdown(&PROG_ID).await {
        Ok(h) => probe(
            &mut passed,
            &mut failed,
            "subscribe_shutdown",
            true,
            &format!("cookie={}", h.cookie),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "subscribe_shutdown",
            false,
            &e.to_string(),
        ),
    }

    // 14. unsubscribe（subscribe → unsubscribe round-trip，验证 IConnectionPoint::Unadvise）。
    match client
        .subscribe(&PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(h) => match client.unsubscribe(h.cookie).await {
            Ok(()) => probe(&mut passed, &mut failed, "unsubscribe", true, "Ok"),
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "unsubscribe",
                false,
                &e.to_string(),
            ),
        },
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "unsubscribe",
            false,
            &e.to_string(),
        ),
    }

    // 15. unsubscribe_shutdown（subscribe_shutdown → unsubscribe_shutdown）。
    match client.subscribe_shutdown(&PROG_ID).await {
        Ok(h) => match client.unsubscribe_shutdown(h.cookie).await {
            Ok(()) => probe(&mut passed, &mut failed, "unsubscribe_shutdown", true, "Ok"),
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "unsubscribe_shutdown",
                false,
                &e.to_string(),
            ),
        },
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "unsubscribe_shutdown",
            false,
            &e.to_string(),
        ),
    }

    // 16. set_subscription_rate（IOPCGroupStateMgt::SetState round-trip，验证 server SetState）。
    match client
        .subscribe(&PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(h) => match client.set_subscription_rate(h.cookie, 1000).await {
            Ok(revised) => probe(
                &mut passed,
                &mut failed,
                "set_subscription_rate",
                revised == 1000,
                &format!("revised={revised}"),
            ),
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "set_subscription_rate",
                false,
                &e.to_string(),
            ),
        },
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "set_subscription_rate",
            false,
            &e.to_string(),
        ),
    }

    // 17. list_servers_with_details（GetClassDetails 填 CLSID/描述）。
    match client.list_servers_with_details(HOST).await {
        Ok(descs) => {
            let found = descs.iter().find(|d| d.prog_id == PROG_ID.as_str());
            probe(
                &mut passed,
                &mut failed,
                "list_servers_with_details",
                found.is_some(),
                &format!("{found:?}"),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "list_servers_with_details",
            false,
            &e.to_string(),
        ),
    }

    // 18. read_tag_values_max_age（预期 Err：server 未实装 IOPCSyncIO2）。
    match client
        .read_tag_values_max_age(&PROG_ID, vec!["Random.Int4".to_string()], 0)
        .await
    {
        Ok(_) => probe(
            &mut passed,
            &mut failed,
            "read_tag_values_max_age",
            false,
            "应失败（server 无 IOPCSyncIO2）但成功了",
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "read_tag_values_max_age",
            true,
            &format!("预期失败: {e}"),
        ),
    }

    // 19. write_tag_value_vqt（预期 Err：server 未实装 IOPCSyncIO2）。
    match client
        .write_tag_value_vqt(
            &PROG_ID,
            "Bucket Brigade.Int4",
            OpcValue::Int(1),
            None,
            None,
        )
        .await
    {
        Ok(_) => probe(
            &mut passed,
            &mut failed,
            "write_tag_value_vqt",
            false,
            "应失败（server 无 IOPCSyncIO2）但成功了",
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "write_tag_value_vqt",
            true,
            &format!("预期失败: {e}"),
        ),
    }

    // 20. set_keep_alive（预期 Err：server 未实装 IOPCGroupStateMgt2）。
    match client
        .subscribe(&PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(h) => match client.set_keep_alive(h.cookie, 5000).await {
            Ok(_) => probe(
                &mut passed,
                &mut failed,
                "set_keep_alive",
                false,
                "应失败（server 无 IOPCGroupStateMgt2）但成功了",
            ),
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "set_keep_alive",
                true,
                &format!("预期失败: {e}"),
            ),
        },
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "set_keep_alive",
            false,
            &e.to_string(),
        ),
    }

    (passed, failed)
}

/// hierarchical 探针（连 GeneratedDataSource 2/2/3=12 leaf server）。`(passed, failed)`。
#[allow(clippy::too_many_lines)]
pub async fn run_hier() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init (hier)");
    let (mut passed, mut failed) = (0u32, 0u32);

    // H1. browse_children(root) → branches（证 QueryOrganization=HIERARCHIAL）。
    let root_branches = match client.browse_children(&PROG_ID, None, 0, 0).await {
        Ok(r) => {
            let ok = !r.branches.is_empty();
            probe(
                &mut passed,
                &mut failed,
                "hier browse_children(root)",
                ok,
                &format!("{} branches", r.branches.len()),
            );
            r.branches
        }
        Err(e) => {
            probe(
                &mut passed,
                &mut failed,
                "hier browse_children(root)",
                false,
                &e.to_string(),
            );
            return (passed, failed);
        }
    };

    // H2. 下钻 plant0 → 应有子节点（line branches，证多层 hierarchical）。
    let plant0_kids = if let Some(b) = root_branches.first() {
        match client
            .browse_children(&PROG_ID, Some(b.id.clone()), 0, 0)
            .await
        {
            Ok(kids) => {
                let ok = !kids.branches.is_empty() || !kids.leaves.is_empty();
                probe(
                    &mut passed,
                    &mut failed,
                    "hier browse_children(plant0)",
                    ok,
                    &format!(
                        "{}: {} branches, {} leaves",
                        b.id,
                        kids.branches.len(),
                        kids.leaves.len()
                    ),
                );
                kids
            }
            Err(e) => {
                probe(
                    &mut passed,
                    &mut failed,
                    "hier browse_children(plant0)",
                    false,
                    &e.to_string(),
                );
                return (passed, failed);
            }
        }
    } else {
        probe(
            &mut passed,
            &mut failed,
            "hier browse_children(plant0)",
            false,
            "root 无 branches",
        );
        return (passed, failed);
    };

    // H3. 下钻 plant0.line0 → leaves 是 full id（client 经 GetItemID 把相对名拼 full path）。
    if let Some(line) = plant0_kids.branches.first() {
        match client
            .browse_children(&PROG_ID, Some(line.id.clone()), 0, 0)
            .await
        {
            Ok(leaves_kids) => {
                let ok = leaves_kids.leaves.first().is_some_and(|leaf| {
                    leaf.item_id.contains('.') && leaf.item_id.starts_with(line.id.as_str())
                });
                probe(
                    &mut passed,
                    &mut failed,
                    "hier leaf full id",
                    ok,
                    &format!(
                        "line={} leaves={} first={:?}",
                        line.id,
                        leaves_kids.leaves.len(),
                        leaves_kids.leaves.first().map(|l| &l.item_id),
                    ),
                );
            }
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "hier leaf full id",
                false,
                &e.to_string(),
            ),
        }
    }

    // H4. browse_tags 全量 → 12 full id（OPC_FLAT fast path / recursive）。
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    match client
        .browse_tags(&PROG_ID, 1000, progress, sink, 0, 0)
        .await
    {
        Ok(tags) => {
            let ok = tags.len() == 12; // 2*2*3
            probe(
                &mut passed,
                &mut failed,
                "hier browse_tags 全量",
                ok,
                &format!("{} full id", tags.len()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "hier browse_tags 全量",
            false,
            &e.to_string(),
        ),
    }

    (passed, failed)
}

/// sim 探针（连 `opc-da-server-sim`，hierarchical 命名空间 + Matrikon 风格 tag 集）。
/// `(passed, failed)`。
///
/// 覆盖 sim 的核心路径：status / read（叶 id `Random.Int4.0`）/ write 往返 /
/// **subscribe 推送（回归：STA client sink 跨线程 QI 的 RPC_E_WRONG_THREAD 修复）** /
/// browse（`Random→Int4` 分支叶数）。sim 默认 count=100 → 801 tag。
#[allow(clippy::too_many_lines)]
pub async fn run_sim() -> (u32, u32) {
    const SIM_PROG_ID: &str = "opc-da-rs.Sim.1";
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init");
    let (mut passed, mut failed) = (0u32, 0u32);

    // S1. get_server_status。
    match client.get_server_status(SIM_PROG_ID).await {
        Ok(s) => probe(
            &mut passed,
            &mut failed,
            "sim get_server_status",
            true,
            &format!("{s:?}"),
        ),
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "sim get_server_status",
            false,
            &e.to_string(),
        ),
    }

    // S2. read Random.Int4.0（hierarchical 叶，quality Good + 值域 0..=100）。
    match client
        .read_tag_values(SIM_PROG_ID, vec!["Random.Int4.0".to_string()])
        .await
    {
        Ok(vals) => {
            let ok = vals.first().is_some_and(|tv| {
                let v: i32 = tv.value.parse().unwrap_or(-1);
                tv.quality == "Good" && (0..=100).contains(&v)
            });
            probe(
                &mut passed,
                &mut failed,
                "sim read Random.Int4.0",
                ok,
                &format!("{:?}", vals.first()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "sim read Random.Int4.0",
            false,
            &e.to_string(),
        ),
    }

    // S3. write BucketBrigade.Int4.0 = 42 → read 回 42（可写 tag 往返）。
    match client
        .write_tag_value(SIM_PROG_ID, "BucketBrigade.Int4.0", OpcValue::Int(42))
        .await
    {
        Ok(_) => match client
            .read_tag_values(SIM_PROG_ID, vec!["BucketBrigade.Int4.0".to_string()])
            .await
        {
            Ok(vals) => {
                let ok = vals
                    .first()
                    .is_some_and(|tv| tv.value.parse::<i32>().unwrap_or(-1) == 42);
                probe(
                    &mut passed,
                    &mut failed,
                    "sim write/read BucketBrigade.Int4.0=42",
                    ok,
                    &format!("{:?}", vals.first()),
                );
            }
            Err(e) => probe(
                &mut passed,
                &mut failed,
                "sim write/read BucketBrigade.Int4.0=42",
                false,
                &e.to_string(),
            ),
        },
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "sim write BucketBrigade.Int4.0",
            false,
            &e.to_string(),
        ),
    }

    // S4. subscribe Random.Int4.0（3s 内收 OnDataChange）——**核心**：验证订阅推送
    //（typed_sinks 免 QI 路径）对 sim 生效（回归 RPC_E_WRONG_THREAD → sinks=0 不推送）。
    match client
        .subscribe(SIM_PROG_ID, vec!["Random.Int4.0".to_string()], 500)
        .await
    {
        Ok(mut handle) => {
            let r = tokio::time::timeout(Duration::from_secs(3), handle.rx.recv()).await;
            let ok = matches!(r, Ok(Some(_)));
            probe(
                &mut passed,
                &mut failed,
                "sim subscribe OnDataChange",
                ok,
                if ok { "收到帧" } else { "3s 未收" },
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "sim subscribe OnDataChange",
            false,
            &e.to_string(),
        ),
    }

    // S5. browse Random→Int4 分支叶数 ≥ 100（默认 count=100 → 801 tag；越界 id 拒收已由
    // read 探针覆盖）。browse_tags 全量应含 hierarchical full id。
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    match client
        .browse_tags(SIM_PROG_ID, 5000, progress, sink, 0, 0)
        .await
    {
        Ok(tags) => {
            let ok = tags.len() > 8 * 100 && tags.contains(&"Random.Int4.0".to_string());
            probe(
                &mut passed,
                &mut failed,
                "sim browse_tags 全量",
                ok,
                &format!("{} full id", tags.len()),
            );
        }
        Err(e) => probe(
            &mut passed,
            &mut failed,
            "sim browse_tags 全量",
            false,
            &e.to_string(),
        ),
    }

    (passed, failed)
}

/// e2e 入口：spawn sim → flat → kill → spawn generated → hierarchical → kill →
/// spawn sim-sim → sim 探针 → kill → 汇总。
pub async fn run_e2e() -> anyhow::Result<()> {
    println!("=== e2e: 全流程（flat + hierarchical + sim）===\n");

    // 阶段 1：SimDataSource server + 13 flat 探针。
    let sim = ServerChild::spawn(&server_exe_path(), "sim", 10, 10, 1000)?;
    // sim 就绪后 SCM 路由到它；稍等 COM 注册完全（缓冲 1s 防 R1 路由竞态）。
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (p1, f1) = run_flat().await;
    drop(sim); // kill sim server。
    // 等 SCM 释放旧实例（防 generated spawn 时 sim 还没完全退出）。
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 阶段 2：GeneratedDataSource server + hierarchical 探针。
    let gen_server = ServerChild::spawn(&server_exe_path(), "generated", 2, 2, 3)?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (p2, f2) = run_hier().await;
    drop(gen_server);

    // 阶段 3：opc-da-server-sim + sim 命名空间探针（read/write/subscribe/browse 全 tag）。
    let sim2 = ServerChild::spawn(&sim_exe_path(), "sim", 0, 0, 0)?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (p3, f3) = run_sim().await;
    drop(sim2);

    let (passed, failed) = (p1 + p2 + p3, f1 + f2 + f3);
    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个探针失败");
    }
    Ok(())
}
