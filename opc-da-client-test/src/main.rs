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
//! # 当前覆盖（M3）
//!
//! - `get_server_status`（`IOPCServer::GetStatus`）。
//! - `read_tag_values`（AddGroup + AddItems + `IOPCSyncIO::Read`）——读 `Random.Int4`，
//!   断言 quality=Good + 值在 read-time 产生器范围 0..=100。
//! - `write_tag_value`（`IOPCSyncIO::Write`）——写 `Bucket Brigade.Int4=42`。
//! - read round-trip——读回 `Bucket Brigade.Int4` 应为 42（Write+Read 闭环）。
//!
//! 后续 milestone 追加 browse / subscribe / item_properties / list_servers。

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider, OpcValue};

/// 自建 server 的 ProgID（`opc-da-server/src/bin/opc-da-server.rs` /RegServer 注册）。
const PROG_ID: &str = "opc-da-rs.Server.1";

#[tokio::main]
#[allow(clippy::too_many_lines)] // 端到端探针线性排列，拆分无收益
async fn main() -> anyhow::Result<()> {
    println!("=== opc-da-client-test: client ↔ opc-da-server 端到端 ===");
    println!("目标 ProgID: {PROG_ID}\n");

    // ComConnector::default() = localhost；OpcDaClient 内部 worker 线程做 CoInitializeEx(MTA)。
    let client = OpcDaClient::new(ComConnector::default())?;
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

    // 5. subscribe（AddGroup + AddItems + FindConnectionPoint[M5a] + Advise DataCallbackSink）。
    //    M5a 验证 advise 链路通（handle 返回 + cookie 非 0）；OnDataChange 推送待 M5b publisher。
    match client
        .subscribe(PROG_ID, vec!["Random.Int4".to_string()], 500)
        .await
    {
        Ok(handle) => {
            println!(
                "✓ subscribe (Random.Int4): cookie={} advise 通 [M5a FindConnectionPoint]",
                handle.cookie
            );
            passed += 1;
        }
        Err(e) => {
            println!("✗ subscribe (Random.Int4): {e}");
            failed += 1;
        }
    }

    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个接口失败");
    }
    Ok(())
}
