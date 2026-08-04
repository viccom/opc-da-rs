//! 压测：M 并发 client 订阅 GeneratedDataSource server + 指标采集。
//!
//! 详见 `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md` §8/§9。
//! v1 矩阵（deadband=0）；v2 deadband 需 client subscribe 暴露 deadband 参数（后续）。
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::duration_suboptimal_units
)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use opc_da_client::{ComConnector, OpcDaClient, OpcProvider};

use crate::server_proc::{ServerChild, read_server_metrics, server_exe_path};

const PROG_ID: &str = "opc-da-rs.Server.1";

/// stress CLI 参数。
pub struct StressOpts {
    pub clients: usize,
    pub items_per_group: usize,
    pub rate: u32,
    pub duration: Duration,
    pub plants: usize,
    pub lines: usize,
    pub sensors: usize,
}

impl Default for StressOpts {
    fn default() -> Self {
        Self {
            clients: 10,
            items_per_group: 100,
            rate: 500,
            duration: Duration::from_secs(60),
            plants: 10,
            lines: 10,
            sensors: 1000,
        }
    }
}

/// 手写解析 `--key value` 参数。
pub fn parse_opts(args: &[String]) -> StressOpts {
    let mut o = StressOpts::default();
    let mut i = 0;
    while i < args.len() {
        let (k, v) = (args[i].as_str(), args.get(i + 1).map(String::as_str));
        let consumed = match (k, v) {
            ("--clients", Some(v)) => {
                o.clients = v.parse().unwrap_or(o.clients);
                true
            }
            ("--items-per-group", Some(v)) => {
                o.items_per_group = v.parse().unwrap_or(o.items_per_group);
                true
            }
            ("--rate", Some(v)) => {
                o.rate = v.parse().unwrap_or(o.rate);
                true
            }
            ("--duration", Some(v)) => {
                o.duration = Duration::from_secs(v.parse().unwrap_or(60));
                true
            }
            ("--plants", Some(v)) => {
                o.plants = v.parse().unwrap_or(o.plants);
                true
            }
            ("--lines", Some(v)) => {
                o.lines = v.parse().unwrap_or(o.lines);
                true
            }
            ("--sensors", Some(v)) => {
                o.sensors = v.parse().unwrap_or(o.sensors);
                true
            }
            _ => false,
        };
        i += if consumed { 2 } else { 1 };
    }
    o
}

/// stress 入口。
pub async fn run_stress(opts: &StressOpts) -> anyhow::Result<()> {
    println!(
        "=== stress: {} clients × {} items, rate={}ms, {}s | generated {}x{}x{} ===",
        opts.clients,
        opts.items_per_group,
        opts.rate,
        opts.duration.as_secs(),
        opts.plants,
        opts.lines,
        opts.sensors,
    );

    let server = ServerChild::spawn(
        &server_exe_path(),
        "generated",
        opts.plants,
        opts.lines,
        opts.sensors,
    )?;
    tokio::time::sleep(Duration::from_secs(1)).await; // COM 注册缓冲。

    let stop = Arc::new(AtomicBool::new(false));
    let total_items = Arc::new(AtomicU64::new(0));
    let total_frames = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for idx in 0..opts.clients {
        let (stop, total_items, total_frames) =
            (stop.clone(), total_items.clone(), total_frames.clone());
        let (ipg, rate) = (opts.items_per_group, opts.rate);
        handles.push(tokio::spawn(async move {
            client_worker(idx, ipg, rate, stop, total_items, total_frames).await
        }));
    }

    // 周期采样（每 30s）输出 server RSS/handles/items 时间序列——判长时泄漏（RSS 持续涨 = 泄漏）。
    let pid = server.pid();
    let mut elapsed = Duration::ZERO;
    while elapsed < opts.duration {
        let chunk = Duration::from_secs(30).min(opts.duration.saturating_sub(elapsed));
        tokio::time::sleep(chunk).await;
        elapsed += chunk;
        let (handles, rss) = read_server_metrics(pid).unwrap_or((0, 0));
        println!(
            "[{:>4}s] clients={} | handles={} RSS={:.1} MB | total items={}",
            elapsed.as_secs(),
            opts.clients,
            handles,
            rss as f64 / 1_048_576.0,
            total_items.load(Ordering::Relaxed),
        );
    }
    stop.store(true, Ordering::Relaxed);

    let mut per_client = Vec::new();
    for h in handles {
        per_client.push(h.await??);
    }

    let items = total_items.load(Ordering::Relaxed);
    let frames = total_frames.load(Ordering::Relaxed);
    stress_summary(
        opts,
        &per_client,
        items,
        frames,
        server.pid(),
        opts.duration,
    );
    Ok(())
}

/// 单 client 线程：subscribe L item，持续计数 OnDataChange 直到 stop。
async fn client_worker(
    idx: usize,
    items_per_group: usize,
    rate: u32,
    stop: Arc<AtomicBool>,
    total_items: Arc<AtomicU64>,
    total_frames: Arc<AtomicU64>,
) -> anyhow::Result<u64> {
    let client = OpcDaClient::new(ComConnector::new("localhost"))?;
    // 取 leaves，按 idx 取 L 个（cycle 兜底防 leaves < clients×items）。
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let all = client
        .browse_tags(PROG_ID, 100_000, progress, sink, 0, 0)
        .await?;
    let start = (idx * items_per_group) % all.len().max(1);
    let items: Vec<String> = all
        .iter()
        .cycle()
        .skip(start)
        .take(items_per_group)
        .cloned()
        .collect();

    let mut handle = client.subscribe(PROG_ID, items, rate).await?;
    let mut mine = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(100), handle.rx.recv()).await
        {
            mine += 1;
            total_items.fetch_add(1, Ordering::Relaxed);
            total_frames.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(mine)
}

/// 输出压测汇总：item/s、帧/s、per-client、server 指标。
fn stress_summary(
    opts: &StressOpts,
    per_client: &[u64],
    items: u64,
    frames: u64,
    pid: u32,
    dur: Duration,
) {
    let secs = dur.as_secs_f64().max(0.001);
    let ips = items as f64 / secs;
    let fps = frames as f64 / secs;
    let min = per_client.iter().copied().min().unwrap_or(0);
    let max = per_client.iter().copied().max().unwrap_or(0);
    let avg = if per_client.is_empty() {
        0.0
    } else {
        items as f64 / per_client.len() as f64
    };
    let (handles, rss) = read_server_metrics(pid).unwrap_or((0, 0));
    println!("\n=== stress 汇总 ===");
    println!(
        "clients: {}  items/group: {}  duration: {:.1}s",
        opts.clients, opts.items_per_group, secs,
    );
    println!("total items: {items}  OnDataChange frames: {frames}");
    println!("item/s: {ips:.0}  frames/s: {fps:.0}");
    println!("per-client items: min={min} max={max} avg={avg:.0}");
    println!(
        "server PID {pid}: handles={handles}  RSS={:.1} MB",
        rss as f64 / 1_048_576.0,
    );
}
