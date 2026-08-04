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

/// 解析 sim server.exe 路径：env `OPC_DA_SIM_EXE` > 默认 `target/debug/opc-da-server-sim.exe`。
/// （sim 独立 crate，与库 bin 不同 exe。）
pub fn sim_exe_path() -> String {
    std::env::var("OPC_DA_SIM_EXE").unwrap_or_else(|_| "target/debug/opc-da-server-sim.exe".into())
}

/// 读 server 子进程指标：`(handle 数, 工作集 RSS 字节)`。
///
/// handle 数近似线程/资源压力；RSS = 物理内存。Windows API 经 PID 打开进程读。
#[cfg(windows)]
#[allow(clippy::cast_possible_truncation)] // size_of usize→u32（cb 字段 API 契约）
pub fn read_server_metrics(pid: u32) -> Result<(u32, usize)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{
        GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: OpenProcess/GetProcessHandleCount/GetProcessMemoryInfo/CloseHandle 为 Windows API；
    // pid 来自 server.pid()（活进程）；pmc zeroed + cb 设 size（API 契约）；句柄 CloseHandle 释放。
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| anyhow::anyhow!("OpenProcess({pid}): {e}"))?;
        let mut handles = 0u32;
        GetProcessHandleCount(h, &raw mut handles)
            .map_err(|e| anyhow::anyhow!("GetProcessHandleCount: {e}"))?;
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(
            h,
            &raw mut pmc,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .map_err(|e| anyhow::anyhow!("GetProcessMemoryInfo: {e}"))?;
        let rss = pmc.WorkingSetSize;
        let _ = CloseHandle(h);
        Ok((handles, rss))
    }
}
