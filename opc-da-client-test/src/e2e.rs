//! 全流程 e2e：13 flat 探针（SimDataSource server）+ hierarchical 探针（GeneratedDataSource）。
//!
//! 流程：spawn sim → 13 flat → kill → spawn generated → hierarchical → kill → 汇总。
//! 详见 `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md` §7。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, OpcValue};

use crate::report::probe;
use crate::server_proc::{ServerChild, server_exe_path};

const PROG_ID: &str = "opc-da-rs.Server.1";
const HOST: &str = "localhost";

/// 13 flat 探针（连 SimDataSource server）。`(passed, failed)`。
///
/// 探针迁移自原 main.rs（get_server_status / read / write / round-trip / subscribe /
/// browse 4 tag / get_item_properties / get_error_string / list_servers / write_tag_values /
/// set_locale_id / set_client_name / subscribe_shutdown）。
#[allow(clippy::too_many_lines)]
pub async fn run_flat() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init");
    let (mut passed, mut failed) = (0u32, 0u32);

    // 1. get_server_status（IOPCServer::GetStatus）。
    match client.get_server_status(PROG_ID).await {
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
        .read_tag_values(PROG_ID, vec!["Random.Int4".to_string()])
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
        .write_tag_value(PROG_ID, "Bucket Brigade.Int4", OpcValue::Int(42))
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
        .read_tag_values(PROG_ID, vec!["Bucket Brigade.Int4".to_string()])
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
        .subscribe(PROG_ID, vec!["Random.Int4".to_string()], 500)
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
    match client.browse_tags(PROG_ID, 100, progress, sink, 0, 0).await {
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
    match client.get_item_properties(PROG_ID, "Random.Int4").await {
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
    match client.get_error_string(PROG_ID, 0).await {
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
            PROG_ID,
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
    match client.set_locale_id(PROG_ID, 0).await {
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
    match client.set_client_name(PROG_ID, "opc-da-client-test").await {
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
    match client.subscribe_shutdown(PROG_ID).await {
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

    (passed, failed)
}

/// hierarchical 探针（连 GeneratedDataSource 2/2/3=12 leaf server）。`(passed, failed)`。
#[allow(clippy::too_many_lines)]
pub async fn run_hier() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init (hier)");
    let (mut passed, mut failed) = (0u32, 0u32);

    // H1. browse_children(root) → branches（证 QueryOrganization=HIERARCHIAL）。
    let root_branches = match client.browse_children(PROG_ID, None, 0, 0).await {
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
            .browse_children(PROG_ID, Some(b.id.clone()), 0, 0)
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
            .browse_children(PROG_ID, Some(line.id.clone()), 0, 0)
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
        .browse_tags(PROG_ID, 1000, progress, sink, 0, 0)
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

/// e2e 入口：spawn sim → 13 flat → kill → spawn generated → hierarchical → kill → 汇总。
pub async fn run_e2e() -> anyhow::Result<()> {
    println!("=== e2e: 全流程（13 flat + hierarchical）===\n");

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

    let (passed, failed) = (p1 + p2, f1 + f2);
    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个探针失败");
    }
    Ok(())
}
