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
//! # 当前覆盖（M1）
//!
//! - `get_server_status`（`IOPCServer::GetStatus`）。
//!
//! 后续 milestone 逐步追加 list / browse / add_group / read / write / subscribe。

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider};

/// 自建 server 的 ProgID（`opc-da-server/src/bin/opc-da-server.rs` /RegServer 注册）。
const PROG_ID: &str = "opc-da-rs.Server.1";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== opc-da-client-test: client ↔ opc-da-server 端到端 ===");
    println!("目标 ProgID: {PROG_ID}\n");

    // ComConnector::default() = localhost；OpcDaClient 内部 worker 线程做 CoInitializeEx(MTA)。
    let client = OpcDaClient::new(ComConnector::default())?;
    let (mut passed, mut failed) = (0u32, 0u32);

    // IOPCServer::GetStatus（server 阶段 0 已实装）。
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

    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个接口失败");
    }
    Ok(())
}
