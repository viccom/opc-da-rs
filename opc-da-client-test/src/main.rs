//! `opc-da-client-test` —— opc-da-client ↔ opc-da-server 端到端测试程序。
//!
//! CLI 自动化：按序调用 opc-da-client 的接口对自建 opc-da-server，每步输出
//! `✓ pass` / `✗ fail (<原因>)`，末尾汇总。用于验收 `opc-da-server` 基础库的
//! 接口完整性（设计 §13 自闭环；计划 `purring-chasing-dusk.md`）。
//!
//! # 前提
//!
//! 1. `opc-da-server.exe /RegServer`（管理员，一次性）注册 ProgID/CLSID/CATID——
//!    `CoCreateInstance` 才能经 ProgID 激活 LocalServer EXE。
//! 2. server EXE 在 `target/{debug,release}/opc-da-server.exe`（SCM 按注册的
//!    `LocalServer32` 路径拉起）。
//!
//! # 用法
//!
//! ```text
//! opc-da-client-test.exe [host]
//! ```
//! - 无参：连本机 localhost（自建 server 本机验证，13 探针）。
//! - `<host>`：DCOM 远程验证——连远程机的 opc-da-server。client 机也需 `/RegServer` 注册
//!   ProgID/CLSID 供 `CLSIDFromProgID` 解析，再 `CoCreateInstanceEx` 连远程 host。
//!
//! # 当前覆盖（M8：全量验证）
//!
//! 13 探针覆盖 opc-da-server 全部已实装接口：
//! - `get_server_status` / `read` / `write` / read round-trip（IOPCServer / ItemMgt / SyncIO）
//! - `subscribe`（FindConnectionPoint + publisher 推送 `OnDataChange`）
//! - `browse`（IOPCBrowseServerAddressSpace）
//! - `get_item_properties`（IOPCItemProperties）
//! - `get_error_string` / `set_locale_id` / `set_client_name`（IOPCCommon）
//! - `list_servers`（CATID 双视图注册）
//! - `write_tag_values`（多 tag write）/ `subscribe_shutdown`（IOPCShutdown cp）
//!
//! 未覆盖：`read_tag_values_max_age` / `write_tag_value_vqt` / `set_keep_alive`
//!（依赖 SyncIO2 / GroupStateMgt2，DA3 阶段 2 未实装）。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, OpcValue};

/// 自建 server 的 ProgID（`opc-da-server/src/bin/opc-da-server.rs` /RegServer 注册）。
const PROG_ID: &str = "opc-da-rs.Server.1";

#[tokio::main]
#[allow(clippy::too_many_lines)] // 端到端探针线性排列，拆分无收益
async fn main() -> anyhow::Result<()> {
    // 命令行第 1 参数 = server host（DCOM 远程验证用）；默认 localhost（本机测试）。
    // 用法：opc-da-client-test.exe [host]  （host 如 192.168.1.10 / server-pc）
    let args: Vec<String> = std::env::args().collect();
    let host = args.get(1).map_or("localhost", String::as_str);
    println!("=== opc-da-client-test: client ↔ opc-da-server 端到端 ===");
    println!("目标 ProgID: {PROG_ID}  host: {host}\n");

    // ComConnector::new(host)：本机 localhost 或远程 host（DCOM）；OpcDaClient 内部 worker 做 CoInitializeEx(MTA)。
    let client = OpcDaClient::new(ComConnector::new(host))?;
    let (mut passed, mut failed) = (0u32, 0u32);

    // 1. IOPCServer::GetStatus（server 阶段 0 已实装）。
    match client.get_server_status(PROG_ID).await {
        Ok(status) => {
            println!("✓ get_server_status: {status:?}");
            passed += 1;
        }
        Err(e) => {
            println!("✗ get_server_status: {e}");
            failed += 1;
        }
    }

    // 2. read Random.Int4（AddGroup[M2] + AddItems[M1] + IOPCSyncIO::Read[M3]）。
    //    read-time 产生器返回 0..=100 的 VT_I4 + GOOD quality。
    match client
        .read_tag_values(PROG_ID, vec!["Random.Int4".to_string()])
        .await
    {
        Ok(vals) => match vals.first() {
            Some(tv) => {
                let v: i32 = tv.value.parse().unwrap_or(-1);
                if tv.quality == "Good" && (0..=100).contains(&v) {
                    println!(
                        "✓ read Random.Int4: value={v} quality={} [M3 Read 通]",
                        tv.quality
                    );
                    passed += 1;
                } else {
                    println!(
                        "✗ read Random.Int4: value={} quality={} (期望 Good + 0..=100)",
                        tv.value, tv.quality
                    );
                    failed += 1;
                }
            }
            None => {
                println!("✗ read Random.Int4: 返回空结果");
                failed += 1;
            }
        },
        Err(e) => {
            println!("✗ read Random.Int4: {e}");
            failed += 1;
        }
    }

    // 3. write Bucket Brigade.Int4 = 42（AddGroup + AddItems + IOPCSyncIO::Write[M3]）。
    match client
        .write_tag_value(PROG_ID, "Bucket Brigade.Int4", OpcValue::Int(42))
        .await
    {
        Ok(_) => {
            println!("✓ write Bucket Brigade.Int4=42: Ok [M3 Write 通]");
            passed += 1;
        }
        Err(e) => {
            println!("✗ write Bucket Brigade.Int4=42: {e}");
            failed += 1;
        }
    }

    // 4. read round-trip：读回 Bucket Brigade.Int4 应为 42（Write+Read 闭环验证）。
    match client
        .read_tag_values(PROG_ID, vec!["Bucket Brigade.Int4".to_string()])
        .await
    {
        Ok(vals) => match vals.first() {
            Some(tv) => {
                if tv.value == "42" {
                    println!(
                        "✓ read round-trip Bucket Brigade.Int4=42: value={} [Write+Read 闭环]",
                        tv.value
                    );
                    passed += 1;
                } else {
                    println!(
                        "✗ read round-trip Bucket Brigade.Int4: value={} (期望 42)",
                        tv.value
                    );
                    failed += 1;
                }
            }
            None => {
                println!("✗ read round-trip Bucket Brigade.Int4: 返回空结果");
                failed += 1;
            }
        },
        Err(e) => {
            println!("✗ read round-trip Bucket Brigade.Int4: {e}");
            failed += 1;
        }
    }

    // 5. subscribe（AddGroup + AddItems + FindConnectionPoint[M5a] + Advise + publisher 推送[M5b]）。
    //    等 OnDataChange 帧（publisher 每 update_rate=500ms 推一帧；3s 内应收到）。
    match client
        .subscribe(PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(mut handle) => {
            match tokio::time::timeout(Duration::from_secs(3), handle.rx.recv()).await {
                Ok(Some(tv)) => {
                    println!(
                        "✓ subscribe 收 OnDataChange: {} value={} quality={} [M5a FindConnectionPoint + M5b publisher]",
                        tv.tag_id, tv.value, tv.quality
                    );
                    passed += 1;
                }
                Ok(None) => {
                    println!("✗ subscribe: rx 关闭，未收数据");
                    failed += 1;
                }
                Err(_) => {
                    println!("✗ subscribe: 3s 内未收 OnDataChange（publisher 未推送）");
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("✗ subscribe (Random.Int4): {e}");
            failed += 1;
        }
    }

    // 6. browse（IOPCBrowseServerAddressSpace[M6]：QueryOrganization=FLAT + BrowseOPCItemIDs
    //    枚举 leaf）。验证列出 SimDataSource 的 4 个 tag。
    let progress = Arc::new(AtomicUsize::new(0));
    let tags_sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    match client
        .browse_tags(PROG_ID, 100, progress, tags_sink, 0, 0)
        .await
    {
        Ok(tags) => {
            let expected = [
                "Random.Int4",
                "Random.Real8",
                "Square Waves.Real8",
                "Bucket Brigade.Int4",
            ];
            let all_found = expected.iter().all(|t| tags.contains(&(*t).to_string()));
            if all_found {
                println!(
                    "✓ browse: 列出 {} tag（含 4 SimDataSource）[M6 BrowseOPCItemIDs]",
                    tags.len()
                );
                passed += 1;
            } else {
                println!("✗ browse: tags={:?}（缺预期 tag）", tags);
                failed += 1;
            }
        }
        Err(e) => {
            println!("✗ browse: {e}");
            failed += 1;
        }
    }

    // 7. get_item_properties（IOPCItemProperties[M7a]：QueryAvailableProperties +
    //    GetItemProperties）。验证 Random.Int4 返回 property 列表（DATATYPE/VALUE/QUALITY/...）。
    match client.get_item_properties(PROG_ID, "Random.Int4").await {
        Ok(props) => {
            if !props.is_empty() {
                println!(
                    "✓ get_item_properties (Random.Int4): {} 个 property [M7a]",
                    props.len()
                );
                passed += 1;
            } else {
                println!("✗ get_item_properties: 返回空");
                failed += 1;
            }
        }
        Err(e) => {
            println!("✗ get_item_properties: {e}");
            failed += 1;
        }
    }

    // 8. get_error_string（IOPCCommon::GetErrorString[M7b]：HRESULT→可读描述）。
    match client.get_error_string(PROG_ID, 0).await {
        Ok(s) => {
            if !s.is_empty() {
                println!("✓ get_error_string (S_OK): {s} [M7b IOPCCommon]");
                passed += 1;
            } else {
                println!("✗ get_error_string: 返回空串");
                failed += 1;
            }
        }
        Err(e) => {
            println!("✗ get_error_string: {e}");
            failed += 1;
        }
    }

    // 9. list_servers（CATID 注册[M7b]：client 经 Implemented Categories 枚举自建 server）。
    match client.list_servers(host).await {
        Ok(servers) => {
            if servers.iter().any(|s| s.contains("opc-da-rs")) {
                println!(
                    "✓ list_servers: 枚举 {} 个 server（含 opc-da-rs）[M7b CATID 注册]",
                    servers.len()
                );
                passed += 1;
            } else {
                println!("✗ list_servers: {:?}（未含 opc-da-rs）", servers);
                failed += 1;
            }
        }
        Err(e) => {
            println!("✗ list_servers: {e}");
            failed += 1;
        }
    }

    // 10. write_tag_values（多 tag write，IOPCSyncIO::Write[M8 扩展覆盖]）。
    match client
        .write_tag_values(
            PROG_ID,
            vec![("Bucket Brigade.Int4".to_string(), OpcValue::Int(99))],
        )
        .await
    {
        Ok(results) => {
            if !results.is_empty() {
                println!(
                    "✓ write_tag_values (Bucket Brigade=99): {} 个结果 [M8]",
                    results.len()
                );
                passed += 1;
            } else {
                println!("✗ write_tag_values: 返回空");
                failed += 1;
            }
        }
        Err(e) => {
            println!("✗ write_tag_values: {e}");
            failed += 1;
        }
    }

    // 11. set_locale_id（IOPCCommon::SetLocaleID[M8]）。
    match client.set_locale_id(PROG_ID, 0).await {
        Ok(()) => {
            println!("✓ set_locale_id (0=system default): Ok [M8 IOPCCommon]");
            passed += 1;
        }
        Err(e) => {
            println!("✗ set_locale_id: {e}");
            failed += 1;
        }
    }

    // 12. set_client_name（IOPCCommon::SetClientName[M8]）。
    match client.set_client_name(PROG_ID, "opc-da-client-test").await {
        Ok(()) => {
            println!("✓ set_client_name: Ok [M8 IOPCCommon]");
            passed += 1;
        }
        Err(e) => {
            println!("✗ set_client_name: {e}");
            failed += 1;
        }
    }

    // 13. subscribe_shutdown（IOPCShutdown cp advise[M8]）。
    match client.subscribe_shutdown(PROG_ID).await {
        Ok(handle) => {
            println!(
                "✓ subscribe_shutdown: cookie={} advise 通 [M8 IOPCShutdown]",
                handle.cookie
            );
            passed += 1;
        }
        Err(e) => {
            println!("✗ subscribe_shutdown: {e}");
            failed += 1;
        }
    }

    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个接口失败");
    }
    Ok(())
}
