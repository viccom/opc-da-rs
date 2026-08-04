//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（需管理员），注册后退出。
//! - `/UnregServer` 清注册项（幂等）。
//! - 无参          启动服务循环。tag 数量优先读 exe 同目录 `opc-da-server-sim.ini`
//!   的 `count = <N>` 行；文件缺失时 fallback env `OPC_DA_SIM_COUNT`；再 fallback 默认 100。
//!
//! Windows-only（依赖 opc-da-server 的 COM 实现）。非 Windows 由库 compile_error! 拒编译。

#[cfg(windows)]
mod data_source;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod tags;
#[cfg(windows)]
mod waveform;

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a.eq_ignore_ascii_case("/RegServer")) {
        return runtime::run_register();
    }
    if args.iter().any(|a| a.eq_ignore_ascii_case("/UnregServer")) {
        return runtime::run_unregister();
    }
    runtime::run_server()
}

/// 初始化 tracing：**stderr + 文件**双写，`DEBUG` 级。
///
/// - **文件**：`<exe 同目录>/logs/opc-da-server-sim.log.YYYY-MM-DD`（每日滚动）。SCM 按
///   `LocalServer32` 拉起 server 时 cwd 是 system32（不可写），日志必须用 `current_exe()`
///   相对路径（与 ini 配置文件同策略）；被 OPC client 拽起时无控制台，文件是唯一日志出口。
/// - **stderr**：手动启动（终端跑 exe）时可见。
///
/// 级别：`DEBUG`（用户要求——诊断 client 互操作问题；库 opc-da-server 的 COM 方法日志
/// 同级别输出）。不用 `EnvFilter`——SCM 启动不继承 shell env，RUST_LOG 不可靠；固定
/// `with_max_level(DEBUG)` 最直接。`try_init` 返 Err 仅当全局 subscriber 已设（重复
/// 初始化），`let _` 吞掉，不 panic。
fn init_tracing() {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let log_dir = crate::runtime::exe_dir().join("logs");
    let file_appender =
        tracing_appender_localtime::rolling::daily(&log_dir, "opc-da-server-sim.log");
    let _ = tracing_subscriber::registry()
        .with(
            // 同步 RollingFileAppender（自身实现 MakeWriter）。不用 non_blocking——Windows
            // 下实测 non_blocking 的 worker 线程丢失日志（缓冲不落盘）；sim 的日志量是
            // COM 方法级（低频），同步写无性能问题。
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_appender)
                .with_filter(LevelFilter::DEBUG),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(LevelFilter::DEBUG),
        )
        .try_init();
}

#[cfg(not(windows))]
fn main() {}
