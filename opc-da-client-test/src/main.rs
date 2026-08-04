//! `opc-da-client-test` —— opc-da-client ↔ opc-da-server 端到端 + 压测程序。
//!
//! 单 binary 双模式（手写 CLI）：
//! - `opc-da-client-test [e2e]`（无参默认）：全流程 e2e（13 flat + hierarchical）。
//! - `opc-da-client-test stress [opts]`：压测（M 并发 client + 指标）。
//!
//! 详见 `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md`。

mod e2e;
mod report;
mod server_proc;
mod stress;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map_or("e2e", String::as_str);
    match sub {
        "e2e" | "" => e2e::run_e2e().await,
        "stress" => stress::run_stress(&stress::parse_opts(&args[2..])).await,
        other => {
            eprintln!("未知子命令: {other}（可用: e2e, stress）");
            std::process::exit(2);
        }
    }
}
