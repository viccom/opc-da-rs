//! 管理 opc-da-server 子进程：spawn（env 选数据源）+ 就绪检测 + Drop kill。
//!
//! e2e/stress 模式用它启动指定数据源的 server 实例（SCM 因子进程已
//! `CoRegisterClassObject` 而路由到它，client 经 ProgID 连入）。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// server 子进程句柄。`Drop` 自动 kill + wait（防泄漏）。
pub struct ServerChild {
    child: Child,
}

impl ServerChild {
    /// spawn `opc-da-server.exe`，设 env 选数据源，等 stderr `serving` 就绪。
    ///
    /// - `datasource`：`sim` / `generated`
    /// - `plants`/`lines`/`sensors`：GeneratedDataSource 规模（sim 时忽略）
    pub fn spawn(
        server_exe: &str,
        datasource: &str,
        plants: usize,
        lines: usize,
        sensors: usize,
    ) -> Result<Self> {
        let mut child = Command::new(server_exe)
            .env("OPC_DA_DATASOURCE", datasource)
            .env("OPC_DA_GEN_PLANTS", plants.to_string())
            .env("OPC_DA_GEN_LINES", lines.to_string())
            .env("OPC_DA_GEN_SENSORS", sensors.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {server_exe} 失败"))?;
        // 就绪检测：读 stderr 直到 "serving"（run_server 的 eprintln）或 10s 超时。
        let stderr = child
            .stderr
            .take()
            .context("stderr 未 piped（spawn 配置错误）")?;
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = String::new();
        loop {
            if Instant::now() > deadline {
                anyhow::bail!("server 子进程 10s 内未就绪（未输出 serving）");
            }
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                anyhow::bail!("server 子进程 stderr 提前关闭（可能启动崩溃）");
            }
            if line.contains("serving") {
                break;
            }
        }
        Ok(Self { child })
    }

    /// server 进程 PID（P4.2 读指标用）。
    #[allow(dead_code)] // P4.2 stress 读指标用
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 解析 server.exe 路径：env `OPC_DA_SERVER_EXE` > 默认 `target/debug/opc-da-server.exe`。
pub fn server_exe_path() -> String {
    std::env::var("OPC_DA_SERVER_EXE").unwrap_or_else(|_| "target/debug/opc-da-server.exe".into())
}
