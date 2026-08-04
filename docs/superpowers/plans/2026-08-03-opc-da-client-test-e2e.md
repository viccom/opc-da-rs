# opc-da-client-test e2e + 压测改造 实现计划（P4）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 改造 `opc-da-client-test` 成单 binary 双模式（`e2e` 全流程 + `stress` 压测），前置让 `opc-da-server` 支持环境变量切 `GeneratedDataSource`。

**Architecture:** 方案 A——server 加 env 切数据源 + `Factory` 注入；client-test 模块化（`main`/`server_proc`/`e2e`/`stress`/`report`），`server_proc` spawn server 子进程（env 选 ds + 就绪检测 + Drop kill）。P4.1 = server 切换 + e2e 全流程（13 flat + hierarchical）；P4.2 = stress 压测（M 并发 client + 指标）。

**Tech Stack:** Rust 2024、`opc-da-client`、`windows-rs`（COM + 进程 API）、`tokio`。

**真相源:** `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md` + `docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md` §P4。

---

## File Structure

**opc-da-server（P4.1 Task 1）**
- Modify `opc-da-server/src/data_source.rs` — 加 `build_data_source()` + `data_source_from_env()`。
- Modify `opc-da-server/src/class_factory.rs` — `Factory` 持 `Arc<dyn DataSource>`；`CreateInstance` 用注入 ds。
- Modify `opc-da-server/src/bin/opc-da-server.rs` — `run_server` 用 `data_source_from_env()` 注入 `Factory`。

**opc-da-client-test（P4.1 Task 2-3 / P4.2 Task 5）**
- Modify `opc-da-client-test/Cargo.toml` — 加 `windows`（P4.2）。
- Create `opc-da-client-test/src/server_proc.rs` — spawn server 子进程 + 就绪 + Drop kill +（P4.2）metrics。
- Create `opc-da-client-test/src/report.rs` — `✓/✗` + 指标输出 helper。
- Create `opc-da-client-test/src/e2e.rs` — 13 flat 探针（迁移自 main.rs）+ hierarchical 探针。
- Create `opc-da-client-test/src/stress.rs`（P4.2）— M 并发 client + 指标采集。
- Rewrite `opc-da-client-test/src/main.rs` — 手写 CLI 解析 + 子命令调度。

**计划文档**
- Modify `docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md` — 勾选 §9 P4 进度。

---

## P4.1 — server 切数据源 + e2e 全流程

### Task 1: opc-da-server env 切数据源 + Factory 注入

**Files:**
- Modify `opc-da-server/src/data_source.rs`（末尾 `mod tests` 前加函数 + 单测）
- Modify `opc-da-server/src/class_factory.rs:19`（Factory struct）+ `:30`（CreateInstance）+ test `:66`
- Modify `opc-da-server/src/bin/opc-da-server.rs:111`（Factory 构造）

- [ ] **Step 1: data_source.rs 加 env 解析（纯函数 + 薄包装）**

在 `data_source.rs` 的 `// —— read-time 值产生器 ——` 注释块**之前**插入：

```rust
// —— 运行时数据源选择（bin run_server 用，P4）——

/// 从进程环境变量构造数据源。
///
/// - `OPC_DA_DATASOURCE=generated` → `GeneratedDataSource`（规模由
///   `OPC_DA_GEN_PLANTS/LINES/SENSORS`，默认 10/10/1000 = 10w leaf）
/// - 缺失 / `sim` / 非法值 → `SimDataSource`（向后兼容已注册常驻 server）
///
/// 读 env 与构造分离（[`build_data_source`] 是纯函数，可单测；本函数薄包装读 env）。
#[must_use]
pub fn data_source_from_env() -> Arc<dyn DataSource> {
    let kind = std::env::var("OPC_DA_DATASOURCE").unwrap_or_default();
    build_data_source(
        &kind,
        parse_env_usize("OPC_DA_GEN_PLANTS").unwrap_or(10),
        parse_env_usize("OPC_DA_GEN_LINES").unwrap_or(10),
        parse_env_usize("OPC_DA_GEN_SENSORS").unwrap_or(1000),
    )
}

/// 按 kind + 规模构造数据源（纯函数，测试用）。
fn build_data_source(
    kind: &str,
    plants: usize,
    lines: usize,
    sensors: usize,
) -> Arc<dyn DataSource> {
    match kind {
        "generated" => Arc::new(GeneratedDataSource::new(plants, lines, sensors)),
        // sim / 空 / 非法 → SimDataSource（向后兼容）。
        _ => Arc::new(SimDataSource::new()),
    }
}

/// 读 env var → `usize`（缺失/非法返 `None`）。
fn parse_env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|s| s.parse().ok())
}
```

- [ ] **Step 2: data_source.rs 加 3 单测（纯函数 `build_data_source`）**

在 `mod tests` 内（`generated_data_source_tree_and_read` 测试后）加：

```rust
    /// `build_data_source`：sim/空/非法 → SimDataSource（flat）；generated → hierarchical。
    #[test]
    fn build_data_source_sim_default_and_unknown() {
        let sim = build_data_source("sim", 10, 10, 1000);
        assert_eq!(sim.query_organization(), NsOrganization::Flat);
        let empty = build_data_source("", 0, 0, 0);
        assert_eq!(empty.query_organization(), NsOrganization::Flat);
        let bogus = build_data_source("bogus", 0, 0, 0);
        assert_eq!(bogus.query_organization(), NsOrganization::Flat, "非法 kind 回退 sim");
    }

    /// `build_data_source("generated", 2,2,3)` → hierarchical + 12 leaf。
    #[test]
    fn build_data_source_generated_hierarchical() {
        let ds = build_data_source("generated", 2, 2, 3);
        assert_eq!(ds.query_organization(), NsOrganization::Hierarchical);
        assert_eq!(ds.namespace().leaves().len(), 12, "2*2*3 = 12 leaf");
    }
```

- [ ] **Step 3: 跑单测验证通过**

Run: `cargo test -p opc-da-server build_data_source`
Expected: PASS（2 测）。

- [ ] **Step 4: class_factory.rs Factory 持 Arc\<dyn DataSource\>**

替换 `class_factory.rs:12-19` 的 import + struct：

```rust
use std::sync::Arc;

use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::core::{BOOL, GUID, IUnknown, Interface, Ref, Result, implement};

use crate::data_source::{DataSource, SimDataSource};
use crate::objects::ServerObj;

/// COM 类工厂——为 `CoCreateInstance` 提供 `ServerObj` 实例。
///
/// 持 `data_source`（bin 启动时注入；`CreateInstance` 用它构造每个 `ServerObj`）。
/// 默认 `SimDataSource`；env `OPC_DA_DATASOURCE=generated` 时为 `GeneratedDataSource`。
#[implement(IClassFactory)]
pub struct Factory {
    data_source: Arc<dyn DataSource>,
}

impl Factory {
    /// 新建 Factory（注入数据源）。bin `run_server` 调。
    pub(crate) fn new(data_source: Arc<dyn DataSource>) -> Self {
        Self { data_source }
    }

    /// 默认 Factory（SimDataSource）。单元测试/兼容用。
    pub(crate) fn default_sim() -> Self {
        Self {
            data_source: Arc::new(SimDataSource::new()),
        }
    }
}
```

- [ ] **Step 5: class_factory.rs CreateInstance 用注入 ds**

替换 `class_factory.rs:30`：

```rust
        let unknown: IUnknown = ServerObj::with_data_source(self.data_source.clone()).into();
```

- [ ] **Step 6: class_factory.rs test 改用 default_sim**

`class_factory.rs` test `self_activate_via_coregister`（约 line 66）把 `let factory: IClassFactory = Factory.into();` 改为：

```rust
            let factory: IClassFactory = Factory::default_sim().into();
```

- [ ] **Step 7: bin/opc-da-server.rs run_server 注入 env 数据源**

`bin/opc-da-server.rs:111`（`let factory: IClassFactory = Factory.into();`）改为：

```rust
        let ds = opc_da_server::data_source::data_source_from_env();
        let factory: IClassFactory = Factory::new(ds).into();
```

（`Factory` 已在 `use opc_da_server::class_factory::{CLSID_OPC_DA_SERVER, Factory}`；`data_source_from_env` 经全路径调。）

- [ ] **Step 8: 质量门 + commit**

Run: `cargo fmt --all && cargo clippy -p opc-da-server --all-targets --all-features -- -D warnings && cargo test -p opc-da-server`
Expected: 全过（含新 2 单测）。

```bash
git add opc-da-server/src/data_source.rs opc-da-server/src/class_factory.rs opc-da-server/src/bin/opc-da-server.rs
git commit -m "feat(opc-da-server): env 切数据源（OPC_DA_DATASOURCE）+ Factory 注入"
```

---

### Task 2: client-test server_proc + report 模块骨架

**Files:**
- Create `opc-da-client-test/src/server_proc.rs`
- Create `opc-da-client-test/src/report.rs`
- Modify `opc-da-client-test/src/main.rs`（加 `mod` 声明；保留旧 13 探针代码暂不动，Task 3 迁移）

- [ ] **Step 1: 创建 server_proc.rs（spawn + 就绪 + Drop kill）**

`opc-da-client-test/src/server_proc.rs`：

```rust
//! 管理 opc-da-server 子进程：spawn（env 选数据源）+ 就绪检测 + Drop kill。
//!
//! e2e/stress 模式用它启动指定数据源的 server 实例（SCM 因子进程已
//! `CoRegisterClassObject` 而路由到它，client 经 ProgID 连入）。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// server 子进程句柄。`Drop` 自动 kill + wait（防泄漏）。
pub(crate) struct ServerChild {
    child: Child,
}

impl ServerChild {
    /// spawn `opc-da-server.exe`，设 env 选数据源，等 stderr `serving` 就绪。
    ///
    /// - `datasource`：`sim` / `generated`
    /// - `plants/lines/sensors`：GeneratedDataSource 规模（sim 时忽略）
    pub(crate) fn spawn(
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
    pub(crate) fn pid(&self) -> u32 {
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
pub(crate) fn server_exe_path() -> String {
    std::env::var("OPC_DA_SERVER_EXE")
        .unwrap_or_else(|_| "target/debug/opc-da-server.exe".into())
}
```

- [ ] **Step 2: 创建 report.rs（✓/✗ helper）**

`opc-da-client-test/src/report.rs`：

```rust
//! 探针结果 + 压测指标输出 helper（共享）。

/// 记一探针结果：pass 则 `passed += 1`，fail 则 `failed += 1`。返回是否通过。
pub(crate) fn probe(passed: &mut u32, failed: &mut u32, name: &str, ok: bool, detail: &str) -> bool {
    if ok {
        println!("✓ {name}: {detail}");
        *passed += 1;
    } else {
        println!("✗ {name}: {detail}");
        *failed += 1;
    }
    ok
}
```

- [ ] **Step 3: main.rs 加 mod 声明**

`opc-da-client-test/src/main.rs` 顶部 `use` 前加：

```rust
mod report;
mod server_proc;
```

（Task 3 再加 `mod e2e;`。）

- [ ] **Step 4: 质量门 + commit**

Run: `cargo fmt --all && cargo clippy -p opc-da-client-test -- -D warnings`
Expected: 全过（server_proc/report 编译，main 暂用旧逻辑不冲突）。

```bash
git add opc-da-client-test/src/server_proc.rs opc-da-client-test/src/report.rs opc-da-client-test/src/main.rs
git commit -m "feat(opc-da-client-test): server_proc + report 模块骨架"
```

---

### Task 3: e2e 模式（13 flat 迁移 + hierarchical 探针）+ main 调度

**Files:**
- Create `opc-da-client-test/src/e2e.rs`
- Rewrite `opc-da-client-test/src/main.rs`（CLI 调度）

- [ ] **Step 1: 创建 e2e.rs——迁移 13 flat 探针**

`opc-da-client-test/src/e2e.rs`。把现 `main.rs:47-343`（`async fn main` 的 13 探针逻辑）迁入新函数 `run_flat`，签名：

```rust
//! 全流程 e2e：13 flat 探针（SimDataSource server）+ hierarchical 探针（GeneratedDataSource）。

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
/// 探针逻辑迁移自原 main.rs（get_server_status / read / write / round-trip /
/// subscribe / browse 4 tag / get_item_properties / get_error_string / list_servers /
/// write_tag_values / set_locale_id / set_client_name / subscribe_shutdown）。
pub(crate) async fn run_flat() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init");
    let (mut passed, mut failed) = (0u32, 0u32);

    // === 迁移原 main.rs:63-337 的 13 探针，每段改用 probe() 记结果 ===
    // 例（1. get_server_status）：
    match client.get_server_status(PROG_ID).await {
        Ok(status) => probe(&mut passed, &mut failed, "get_server_status", true, &format!("{status:?}")),
        Err(e) => probe(&mut passed, &mut failed, "get_server_status", false, &e.to_string()),
    }
    // ... 其余 12 探针同模式迁移（read/write/round-trip/subscribe/browse 4 tag/
    //     get_item_properties/get_error_string/list_servers/write_tag_values/
    //     set_locale_id/set_client_name/subscribe_shutdown）。逻辑体原样搬，
    //     只把 println!("✓/✗ ...") 换成 probe(&mut passed, &mut failed, name, ok, detail)。
    //     其中探针 6（browse 4 tag）的断言 expected 4 tag 保留。

    (passed, failed)
}
```

> **迁移说明**（避免逐行复制 280 行）：原 `main.rs` 13 个 `match` 块的**业务逻辑体**（API 调用 + 断言）原样搬到 `run_flat`，每个块的 `println!("✓...")` / `println!("✗...")` 两分支合并为一个 `probe(&mut passed, &mut failed, "<name>", <ok 条件>, &<detail>)` 调用。例如探针 2（read Random.Int4）：

```rust
    // 2. read Random.Int4
    match client.read_tag_values(PROG_ID, vec!["Random.Int4".to_string()]).await {
        Ok(vals) => {
            let ok = vals.first().map_or(false, |tv| {
                let v: i32 = tv.value.parse().unwrap_or(-1);
                tv.quality == "Good" && (0..=100).contains(&v)
            });
            let detail = format!("{:?}", vals.first());
            probe(&mut passed, &mut failed, "read Random.Int4", ok, &detail);
        }
        Err(e) => probe(&mut passed, &mut failed, "read Random.Int4", false, &e.to_string()),
    }
```

> 其余 11 探针（3 write / 4 round-trip / 5 subscribe / 6 browse 4 tag / 7 get_item_properties / 8 get_error_string / 9 list_servers / 10 write_tag_values / 11 set_locale_id / 12 set_client_name / 13 subscribe_shutdown）按同一模式迁移，断言条件不变。

- [ ] **Step 2: e2e.rs 加 hierarchical 探针（GeneratedDataSource server）**

在 `e2e.rs` 加 `run_hier` + `run_e2e`：

```rust
/// hierarchical 探针（连 GeneratedDataSource 2/2/3=12 leaf server）。`(passed, failed)`。
pub(crate) async fn run_hier() -> (u32, u32) {
    let client = OpcDaClient::new(ComConnector::new(HOST)).expect("OpcDaClient init (hier)");
    let (mut passed, mut failed) = (0u32, 0u32);

    // H1. browse_children(root) → branches（证 QueryOrganization=HIERARCHIAL）
    let root = client.browse_children(PROG_ID, None, 0, 0).await;
    let root_branches = match root {
        Ok(r) => {
            let ok = !r.branches.is_empty();
            probe(&mut passed, &mut failed, "hier browse_children(root)",
                  ok, &format!("{} branches", r.branches.len()));
            r.branches
        }
        Err(e) => {
            probe(&mut passed, &mut failed, "hier browse_children(root)", false, &e.to_string());
            return (passed, failed);
        }
    };

    // H2. 下钻第一个 branch → 应有子节点（branches 或 leaves）
    if let Some(b) = root_branches.first() {
        match client.browse_children(PROG_ID, Some(&b.id), 0, 0).await {
            Ok(kids) => {
                let ok = !kids.branches.is_empty() || !kids.leaves.is_empty();
                probe(&mut passed, &mut failed, "hier browse_children(下钻)",
                      ok, &format!("{}: {} branches, {} leaves", b.id, kids.branches.len(), kids.leaves.len()));
                // H3. 下钻的 leaves 应是 full id（client 经 GetItemID 拼好）
                if let Some(leaf) = kids.leaves.first() {
                    let ok = !leaf.item_id.is_empty() && leaf.item_id.contains('.');
                    probe(&mut passed, &mut failed, "hier leaf full id",
                          ok, &format!("name={:?} item_id={:?}", leaf.name, leaf.item_id));
                }
            }
            Err(e) => probe(&mut passed, &mut failed, "hier browse_children(下钻)", false, &e.to_string()),
        }
    }

    // H4. browse_tags 全量 → 12 full id（OPC_FLAT fast path）
    let progress = Arc::new(AtomicUsize::new(0));
    let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    match client.browse_tags(PROG_ID, 1000, progress, sink, 0, 0).await {
        Ok(tags) => {
            let ok = tags.len() == 12; // 2*2*3
            probe(&mut passed, &mut failed, "hier browse_tags 全量",
                  ok, &format!("{} full id", tags.len()));
        }
        Err(e) => probe(&mut passed, &mut failed, "hier browse_tags 全量", false, &e.to_string()),
    }

    (passed, failed)
}

/// e2e 入口：spawn sim → 13 flat → kill → spawn generated → hierarchical → kill → 汇总。
pub(crate) async fn run_e2e() -> anyhow::Result<()> {
    println!("=== e2e: 全流程（13 flat + hierarchical）===\n");

    // 阶段 1：SimDataSource server + 13 flat 探针。
    let _sim = ServerChild::spawn(&server_exe_path(), "sim", 10, 10, 1000)?;
    // sim 就绪后 SCM 路由到它；稍等 COM 注册完全（缓冲 1s 防 R1 竞态）。
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (p1, f1) = run_flat().await;
    drop(_sim); // kill sim server。
    // 等 SCM 释放旧实例（防 generated spawn 时 sim 还没完全退出）。
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 阶段 2：GeneratedDataSource server + hierarchical 探针。
    let _gen = ServerChild::spawn(&server_exe_path(), "generated", 2, 2, 3)?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (p2, f2) = run_hier().await;
    drop(_gen);

    let (passed, failed) = (p1 + p2, f1 + f2);
    println!("\n=== 汇总: {passed} passed, {failed} failed ===");
    if failed > 0 {
        anyhow::bail!("{failed} 个探针失败");
    }
    Ok(())
}
```

- [ ] **Step 3: 重写 main.rs（CLI 调度）**

`opc-da-client-test/src/main.rs` 全文替换为：

```rust
//! `opc-da-client-test` —— opc-da-client ↔ opc-da-server 端到端 + 压测程序。
//!
//! 单 binary 双模式（手写 CLI）：
//! - `opc-da-client-test [e2e]`（无参默认）：全流程 e2e（13 flat + hierarchical）。
//! - `opc-da-client-test stress [opts]`：压测（P4.2）。
//!
//! 详见 `docs/superpowers/specs/2026-08-03-opc-da-client-test-e2e-design.md`。

mod e2e;
mod report;
mod server_proc;
// P4.2 启用：mod stress;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(String::as_str).unwrap_or("e2e");
    match sub {
        "e2e" | "" => e2e::run_e2e().await,
        // "stress" => stress::run_stress(&stress::parse_opts(&args[2..])).await, // P4.2
        other => {
            eprintln!("未知子命令: {other}（可用: e2e, stress[P4.2]）");
            std::process::exit(2);
        }
    }
}
```

（`mod stress;` 与 stress 分支注释，P4.2 Task 5 启用。）

- [ ] **Step 4: 跑 e2e 验证**

需 server.exe 已编译 + `/RegServer` 已注册（让 SCM 知 CLSID→exe）。先确保：

```bash
cargo build -p opc-da-server
./target/debug/opc-da-server.exe /RegServer   # 一次性注册（管理员）
```

Run: `cargo run -p opc-da-client-test`
Expected: 输出 13 flat + hierarchical 探针，全部 `✓`，末尾 `汇总: N passed, 0 failed`，exit 0。

> 若 hierarchical 探针失败（SCM 路由到旧 sim 实例）：增大 `_sim` drop 后的 sleep（R1 缓解），或确认 `/RegServer` 已注册。若 browse_children 在 GeneratedDataSource 上返空 branches，查 server env 是否生效（stderr 应无 `OPC_DA_DATASOURCE` 解析错误）。

- [ ] **Step 5: 质量门 + commit**

Run: `cargo fmt --all && cargo clippy -p opc-da-client-test -- -D warnings && cargo run -p opc-da-client-test`
Expected: clippy 干净 + e2e 全 pass exit 0。

```bash
git add opc-da-client-test/src/e2e.rs opc-da-client-test/src/main.rs
git commit -m "feat(opc-da-client-test): e2e 全流程（13 flat + hierarchical browse 探针）"
```

---

### Task 4: P4.1 质量门 + 勾选

**Files:**
- Modify `docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md`（§9 P4 + P2 hierarchical e2e 项）

- [ ] **Step 1: 完整质量门**

Run: `pwsh -File scripts/verify.ps1`
Expected: 全门过，13 探针（现 e2e 模式）+ hierarchical 全 pass exit 0。

> verify.ps1 流程不变（`/RegServer` → `cargo build -p opc-da-server` → `cargo run -p opc-da-client-test`）。client-test 自己 spawn sim + generated server。

- [ ] **Step 2: 勾选 scale-plan §9**

`scale-plan.md` §9 P4 checklist：勾选 P4.1 相关项（server 切换 + client-test 模块化 + e2e + hierarchical 探针扩展）；P4.2（stress）保持未勾。同时勾选 P2 的「opc-da-client-test browse 探针扩展（hierarchical）」（本设计 P4.1 完成）。

- [ ] **Step 3: commit 勾选**

```bash
git add docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md
git commit -m "chore(scale-plan): 勾选 P4.1（e2e 全流程 + hierarchical 探针）完成"
git push origin feat/opc-da-server
```

---

## P4.2 — stress 压测

### Task 5: stress 模式 + 指标采集

**Files:**
- Modify `opc-da-client-test/Cargo.toml`（加 windows）
- Modify `opc-da-client-test/src/server_proc.rs`（加 metrics 读取）
- Create `opc-da-client-test/src/stress.rs`
- Modify `opc-da-client-test/src/report.rs`（加 stress 输出）
- Modify `opc-da-client-test/src/main.rs`（启用 stress 子命令）

- [ ] **Step 1: Cargo.toml 加 windows**

`opc-da-client-test/Cargo.toml` `[dependencies]` 加（client-test 仅 Windows，不需 cfg gating）：

```toml
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_System_ProcessStatus",
] }
```

> 若 `Win32_System_ProcessStatus`（`GetProcessMemoryInfo`）feature 名编译报错，查 windows-rs 文档替换（备选 `Win32_System_Diagnostics_ToolHelp`）。

- [ ] **Step 2: server_proc.rs 加 metrics 读取**

`server_proc.rs` 末尾加：

```rust
/// 读 server 子进程指标：(handle 数, 工作集 RSS 字节)。
///
/// handle 数近似线程/资源压力；RSS = 物理内存。Windows API 经 PID 打开进程读。
#[cfg(windows)]
pub(crate) fn read_server_metrics(pid: u32) -> Result<(u32, usize)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| anyhow::anyhow!("OpenProcess({pid}): {e}"))?;
        let mut handles = 0u32;
        GetProcessHandleCount(h, &mut handles)
            .map_err(|e| anyhow::anyhow!("GetProcessHandleCount: {e}"))?;
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        GetProcessMemoryInfo(h, &mut pmc, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32)
            .map_err(|e| anyhow::anyhow!("GetProcessMemoryInfo: {e}"))?;
        let rss = pmc.WorkingSetSize;
        let _ = CloseHandle(h);
        Ok((handles, rss))
    }
}
```

> `PROCESS_MEMORY_COUNTERS` 默认值用 `::default()`（需 `Default`；windows-rs 结构体派生）。若未派生，改 `PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() }` 或 `zeroed()`。

- [ ] **Step 3: 创建 stress.rs**

`opc-da-client-test/src/stress.rs`：

```rust
//! 压测：M 并发 client 订阅 GeneratedDataSource server + 指标采集。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use opc_da_client::{ComConnector, OpcDaClient, OpcProvider};

use crate::report;
use crate::server_proc::{ServerChild, read_server_metrics, server_exe_path};

const PROG_ID: &str = "opc-da-rs.Server.1";

/// stress CLI 参数。
pub(crate) struct StressOpts {
    pub clients: usize,
    pub items_per_group: usize,
    pub rate: u32,
    pub deadband: f32,
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
            deadband: 0.0,
            duration: Duration::from_secs(60),
            plants: 10,
            lines: 10,
            sensors: 1000,
        }
    }
}

/// 手写解析 `--key value` 参数（stress 子命令）。
pub(crate) fn parse_opts(args: &[String]) -> StressOpts {
    let mut o = StressOpts::default();
    let mut i = 0;
    while i < args.len() {
        let (k, v) = (args[i].as_str(), args.get(i + 1));
        match (k, v) {
            ("--clients", Some(v)) => { o.clients = v.parse().unwrap_or(o.clients); i += 2; }
            ("--items-per-group", Some(v)) => { o.items_per_group = v.parse().unwrap_or(o.items_per_group); i += 2; }
            ("--rate", Some(v)) => { o.rate = v.parse().unwrap_or(o.rate); i += 2; }
            ("--deadband", Some(v)) => { o.deadband = v.parse().unwrap_or(o.deadband); i += 2; }
            ("--duration", Some(v)) => { o.duration = Duration::from_secs(v.parse().unwrap_or(60)); i += 2; }
            ("--plants", Some(v)) => { o.plants = v.parse().unwrap_or(o.plants); i += 2; }
            ("--lines", Some(v)) => { o.lines = v.parse().unwrap_or(o.lines); i += 2; }
            ("--sensors", Some(v)) => { o.sensors = v.parse().unwrap_or(o.sensors); i += 2; }
            _ => { i += 1; }
        }
    }
    o
}

/// stress 入口。
pub(crate) async fn run_stress(opts: &StressOpts) -> Result<()> {
    println!("=== stress: {} clients × {} items, rate={}ms, deadband={}, {}s ===",
             opts.clients, opts.items_per_group, opts.rate, opts.deadband, opts.duration.as_secs());

    let _server = ServerChild::spawn(&server_exe_path(), "generated",
                                      opts.plants, opts.lines, opts.sensors)?;
    tokio::time::sleep(Duration::from_secs(1)).await; // COM 注册缓冲。

    let stop = Arc::new(AtomicBool::new(false));
    let total_items = Arc::new(AtomicU64::new(0));
    let total_frames = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for idx in 0..opts.clients {
        let (stop, total_items, total_frames) = (stop.clone(), total_items.clone(), total_frames.clone());
        handles.push(tokio::spawn(async move {
            client_worker(idx, opts, stop, total_items, total_frames).await
        }));
    }

    tokio::time::sleep(opts.duration).await;
    stop.store(true, Ordering::Relaxed);

    let mut per_client = Vec::new();
    for h in handles {
        per_client.push(h.await?);
    }

    let items = total_items.load(Ordering::Relaxed);
    let frames = total_frames.load(Ordering::Relaxed);
    let pid = _server.pid();
    report::stress_summary(opts, &per_client, items, frames, pid, opts.duration);
    Ok(())
}

/// 单 client 线程：subscribe L item，持续计数 OnDataChange 直到 stop。
async fn client_worker(
    idx: usize,
    opts: &StressOpts,
    stop: Arc<AtomicBool>,
    total_items: Arc<AtomicU64>,
    total_frames: Arc<AtomicU64>,
) -> Result<u64> {
    let client = OpcDaClient::new(ComConnector::new("localhost"))?;
    // 选 items：browse_tags 取 leaves，按 idx 取 L 个（避免全 client 同 item）。
    let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sink: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let all = client.browse_tags(PROG_ID, 100_000, progress, sink, 0, 0).await?;
    let start = (idx * opts.items_per_group) % all.len().max(1);
    let items: Vec<String> = all.iter().cycle().skip(start).take(opts.items_per_group).cloned().collect();

    let mut handle = client.subscribe(PROG_ID, items, opts.rate).await?;
    let mut mine = 0u64;
    while !stop.load(Ordering::Relaxed) {
        match tokio::time::timeout(Duration::from_millis(100), handle.rx.recv()).await {
            Ok(Some(_)) => {
                mine += 1;
                total_items.fetch_add(1, Ordering::Relaxed);
                total_frames.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    Ok(mine)
}

/// server metrics 读取（server_proc），report 里调。
#[allow(dead_code)]
fn metrics(pid: u32) -> (u32, usize) {
    read_server_metrics(pid).unwrap_or((0, 0))
}
```

- [ ] **Step 4: report.rs 加 stress_summary**

`report.rs` 末尾加：

```rust
use std::time::Duration;

use crate::server_proc::read_server_metrics;
use crate::stress::StressOpts;

/// 输出压测汇总：item/s、帧/s、per-client、server 指标。
pub(crate) fn stress_summary(opts: &StressOpts, per_client: &[u64], items: u64, frames: u64, pid: u32, dur: Duration) {
    let secs = dur.as_secs_f64().max(0.001);
    let ips = items as f64 / secs;
    let fps = frames as f64 / secs;
    let min = per_client.iter().copied().min().unwrap_or(0);
    let max = per_client.iter().copied().max().unwrap_or(0);
    let avg = if per_client.is_empty() { 0.0 } else { items as f64 / per_client.len() as f64 };
    let (handles, rss) = read_server_metrics(pid).unwrap_or((0, 0));
    println!("\n=== stress 汇总 ===");
    println!("clients: {}  items/group: {}  duration: {:.1}s", opts.clients, opts.items_per_group, secs);
    println!("total items: {items}  OnDataChange frames: {frames}");
    println!("item/s: {ips:.0}  frames/s: {fps:.0}");
    println!("per-client items: min={min} max={max} avg={avg:.0}");
    println!("server PID {pid}: handles={handles}  RSS={:.1} MB", rss as f64 / 1_048_576.0);
}
```

> `report.rs` 顶部 `use crate::stress::StressOpts` 引入 stress 类型——`stress` mod 在 main 声明。注意循环依赖（report↔stress）：`StressOpts` 是数据类型，report 引用其类型注解；若编译报循环，把 `stress_summary` 的 `opts` 参数改为基本类型（clients/items_per_group/duration 直接传）避免 `use crate::stress`。

- [ ] **Step 5: main.rs 启用 stress 子命令**

`main.rs` 改：

```rust
mod e2e;
mod report;
mod server_proc;
mod stress;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(String::as_str).unwrap_or("e2e");
    match sub {
        "e2e" | "" => e2e::run_e2e().await,
        "stress" => stress::run_stress(&stress::parse_opts(&args[2..])).await,
        other => {
            eprintln!("未知子命令: {other}（可用: e2e, stress）");
            std::process::exit(2);
        }
    }
}
```

- [ ] **Step 6: 跑 stress 验证**

Run: `cargo run -p opc-da-client-test stress --clients 5 --duration 10 --plants 2 --lines 2 --sensors 100`
Expected: 输出 stress 汇总（item/s、frames/s、per-client、server handles/RSS），无 panic，exit 0。

> 小规模（5 client / 10s）先验证框架跑通；v1 矩阵（100 client）在 Task 6。

- [ ] **Step 7: 质量门 + commit**

Run: `cargo fmt --all && cargo clippy -p opc-da-client-test -- -D warnings`
Expected: 干净。

```bash
git add opc-da-client-test/
git commit -m "feat(opc-da-client-test): stress 压测模式（M 并发 client + 指标采集）"
```

---

### Task 6: v1 矩阵验证 + P4.2 勾选

**Files:**
- Modify `docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md`（§9 P4 + §10 压测结果）

- [ ] **Step 1: 跑 v1 矩阵**

Run: `cargo run -p opc-da-client-test stress --clients 100 --items-per-group 100 --rate 500 --duration 60`
Expected: 60s 稳定，输出指标。**达标线**（scale-plan §P4 v1）：
- server handles ≤ `核数 × 2 + 常数`（P0 统一调度后线程数稳定）
- 推送稳定无丢（item/s > 0，持续）
- 60s 无 OOM（RSS 不爆涨）

> 记录 item/s、frames/s、handles、RSS 到 §10。

- [ ] **Step 2: 勾选 scale-plan §9 P4 + 填 §10 结果**

`scale-plan.md` §9 P4 勾选 stress 工具 / mock client / v1 矩阵；§10 填 v1 行（日期、commit、场景、item/s 等）。v2/v3 待后续（deadband / 10w）。

- [ ] **Step 3: commit + push**

```bash
git add docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md
git commit -m "chore(scale-plan): 勾选 P4.2（stress 压测）+ v1 矩阵结果"
git push origin feat/opc-da-server
```

---

## 完成标准

- P4.1：`cargo run -p opc-da-client-test`（e2e）→ 13 flat + hierarchical 全 pass exit 0；`verify.ps1` 全门过。
- P4.2：`cargo run -p opc-da-client-test stress --clients 100 --duration 60` → v1 达标（线程稳定、推送稳定、无 OOM）。

## 风险与回退

- **R1（SCM 路由竞态）**：e2e/stress spawn server 后 client 连太快 → SCM 另启实例。缓解：`ServerChild::spawn` 等就绪 + spawn 后 sleep 1s。若仍路由错，增大 sleep 或查 `/RegServer` 注册。
- **R2（windows feature 名）**：`GetProcessMemoryInfo` 的 feature 若非 `Win32_System_ProcessStatus`，编译报错时查 windows-rs 文档替换（R3）。
- **R3（subscribe 在 GeneratedDataSource）**：item 按 idx 取模分配，若某些 client 拿到空 items（all.len() < items_per_group × clients），cycle 兜底（重复 item，可接受压测）。
