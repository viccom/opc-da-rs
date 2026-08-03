# opc-da-client-test 全流程 e2e + 压测改造设计（P4）

> 日期：2026-08-03　分支：`feat/opc-da-server`
> 状态：设计定稿，待实现　真相源：本文件 + `scale-plan.md` §P4
> 上游：`scale-plan.md` §P4（压测基础设施）、§P2（hierarchical，P2.3 留 P4 的 e2e 探针）

---

## 0. 文档定位

`scale-plan.md` §P4 原写"新建 `stress.rs` 或独立 crate"。本设计调整为**改造现有 `opc-da-client-test`** 成 e2e + 压测统一载体（用户决策 2026-08-03：方案 A——单 binary + 子命令 + 模块化）。本文件是 P4 详细设计；实现 plan 基于本文件 + scale-plan §P4。

---

## 1. 目标

- **全流程 e2e**：13 flat 探针（现有）+ hierarchical browse 探针（P2.3 遗留），覆盖 OPC DA 全接口路径（connect / browse / read / write / subscribe / shutdown），跨进程验证 client↔server 互操作。
- **压测基础设施**：M 并发 client 订阅/读 + 指标采集 + v1/v2/v3 矩阵，验证 scale-plan §1.1 目标（100+ client / <100w 订阅 / 10w item）。
- **单 binary 两模式**：`opc-da-client-test`（无参 = e2e）/ `stress`，verify.ps1 无参兼容（exit 0）。

---

## 2. 现状

- `opc-da-client-test`（workspace 成员）：单文件 `main.rs` 344 行，13 探针线性排列，连 SCM 拉起的 opc-da-server（SimDataSource flat）。verify.ps1 `cargo run -p opc-da-client-test` 期望 exit 0。
- 缺失：hierarchical browse 探针（P2.3 单测覆盖 server 侧逻辑，跨进程 e2e 留 P4）。
- server 写死 SimDataSource（`ServerObj::new()`），无运行时切 GeneratedDataSource 入口（P2.3 加了 `with_data_source` 但 factory 未用）。
- client 端 `OpcProvider::browse_children` 已封装 hierarchical 树导航（DOWN/UP/GetItemID），e2e 探针可直接复用。

---

## 3. 架构（方案 A：单 binary + 子命令 + 模块化）

```
opc-da-client-test/src/
├── main.rs        # CLI 解析（手写 std::env::args）+ 子命令调度
├── server_proc.rs # spawn opc-da-server.exe（env 选 ds）+ 就绪检测 + Drop kill
├── e2e.rs         # 全流程探针：13 flat + hierarchical
├── stress.rs      # M 并发 client + 指标采集 + 矩阵
└── report.rs      # ✓/✗ + 压测指标输出（共享 helper）
```

单 crate 单 bin。CLI 手写 `std::env::args` 解析（子命令 + 几个 flag，避免引入 clap 新依赖——简洁优先 / YAGNI）。

---

## 4. 前置改动（opc-da-server）

让 server 支持运行时切数据源（P4.1 第一步）：

- `bin/opc-da-server.rs` `run_server` 读 env 选数据源：
  - `OPC_DA_DATASOURCE`：`sim`（默认）/ `generated`
  - `OPC_DA_GEN_PLANTS` / `OPC_DA_GEN_LINES` / `OPC_DA_GEN_SENSORS`：GeneratedDataSource 规模（默认 10/10/1000 = 10w leaf）
- `Factory` 改持 `Arc<dyn DataSource>`（bin 启动时构造一次注入 factory）；`CreateInstance` 用注入 ds 调 `ServerObj::with_data_source(ds)`（复用 P2.3）。
- env 缺失/非法值 → 回退 SimDataSource（向后兼容现有注册常驻 server，verify.ps1 不受影响）。

---

## 5. 子命令 CLI（手写解析）

- `opc-da-client-test [e2e]`（无参默认 e2e）：全流程 e2e。
- `opc-da-client-test stress [opts]`：压测。
- **stress opts**：`--clients N`（默认 10）/ `--items-per-group N`（默认 100）/ `--rate MS`（默认 500）/ `--deadband PCT`（默认 0.0）/ `--duration S`（默认 60）/ `--plants N` / `--lines N` / `--sensors N`（GeneratedDataSource 规模，默认 10/10/1000）。
- **共享 opt**：`--server-exe <path>`（opc-da-server.exe 路径，默认 env `OPC_DA_SERVER_EXE` 或 `target/debug/opc-da-server.exe`）。

---

## 6. server_proc.rs（子进程管理）

- `spawn(ds, gen_params) -> ServerChild`：`Command::new(server_exe)`，设 env（`OPC_DA_DATASOURCE` + `OPC_DA_GEN_*`），`spawn()`，捕获 stderr 管道。
- 就绪检测：读 stderr 直到 `opc-da-server: serving`（`run_server` 已打印）或超时（10s → bail）。
- `ServerChild` 实现 `Drop`：`child.kill()` + `wait()`（防泄漏，即使探针 panic/err 也清理）。
- 路径解析：env `OPC_DA_SERVER_EXE` > 默认 `target/debug/opc-da-server.exe`（相对 cwd）。

---

## 7. e2e 模式流程

1. spawn SimDataSource server（env `OPC_DA_DATASOURCE=sim`）→ 等就绪。
2. **13 flat 探针**（现有逻辑从 main.rs 迁到 `e2e.rs`，连 localhost ProgID）：get_server_status / read / write / round-trip / subscribe / browse(4 tag) / get_item_properties / get_error_string / list_servers / write_tag_values / set_locale_id / set_client_name / subscribe_shutdown。
3. kill SimDataSource server（`ServerChild` drop）。
4. spawn GeneratedDataSource server（env `generated`，**e2e 用小规模 2/2/3 = 12 leaf** 加速；10w 留 stress）→ 等就绪。
5. **hierarchical 探针**（用 `client.browse_children`，client 已封装 DOWN/UP/GetItemID）：
   - `browse_children(root)` → branches `[plant0, plant1]`（非空，证 QueryOrganization=HIERARCHIAL）
   - `browse_children("plant0")` → branches `[line0, line1]`
   - `browse_children("plant0.line0")` → leaves `[plant0.line0.sensor0..2]`（full id，证 GetItemID 相对名→full 链路）
   - `browse_tags` → 全量 12 full id（OPC_FLAT fast path 或 recursive）
6. kill GeneratedDataSource server。
7. 汇总（13 flat + hierarchical），全 pass `exit 0`，否则 `bail!`。

---

## 8. stress 模式流程（P4.2）

1. spawn GeneratedDataSource server（规模按 `--plants/--lines/--sensors`）→ 等就绪。
2. spawn M 个 client 线程（每线程独立 `OpcDaClient`）：
   - connect（ProgID @ localhost）
   - AddGroup（`rate`, `deadband`）+ AddItems（L 个 item，从 GeneratedDataSource leaves 按 hash 取模分配，避免全 client 订阅同 item）
   - subscribe（OnDataChange 流）
   - 持续 `duration`：原子计数器累加收到 item 数 + OnDataChange 帧数
3. 主线程 `duration` 秒后发停止信号（`Arc<AtomicBool>`），join 各线程收集计数。
4. 指标采集（见 §9）+ 报告。
5. kill server。

---

## 9. 指标采集

- **client 侧**（`AtomicU64` 计数器，每线程累加）：
  - 总 item/s（所有线程收到 item 总数 / duration）
  - 总 OnDataChange 帧/s
  - 每 client item/s（min / max / avg）
  - 推送间隔：OnDataChange 帧到达间隔（client 本地时钟测；client 拿不到 server read ts，故不测端到端延迟）
- **server 侧**（client-test 经 Windows API 读 server 子进程 PID）：
  - 线程数：`GetProcessHandleCount`（handle 数近似线程级压力；精确线程数需 NtQuery，YAGNI 先用 handle 数）
  - 内存 RSS：`GetProcessMemoryInfo`（`PROCESS_MEMORY_COUNTERS::WorkingSetSize`）
  - CPU 可选：`GetProcessTimes`
- client-test `Cargo.toml` 加 windows features：`Win32_System_Threading` + `Win32_System_ProcessStatus`（GetProcessMemoryInfo；feature 名实现时查证）。

---

## 10. 压测矩阵（引用 scale-plan §P4）

stress CLI 参数化跑矩阵：

| 版本 | 场景 | 达标线 |
|---|---|---|
| **v1** | 1w 组 / 100 client / 各 100 item / deadband=0 / 60s | 线程数 ≤ 核数×2+常数；推送稳定无丢；60s 无 OOM |
| **v2** | + deadband 5% + 30% 变化率数据 | OnDataChange/s vs deadband=0 降 1 量级 |
| **v3** | 10w item 树 / 100 client / 总订阅 50w / deadband 5% / 60s | browse 10w < 2s；100 client 稳定 |

v3 达标 = scale-plan §1.1 目标完成。余量测试输出瓶颈报告，指导 P3。

---

## 11. 向后兼容（verify.ps1）

- verify.ps1 流程不变：`/RegServer`（注册 CLSID→exe 映射）→ `cargo build -p opc-da-server` → `cargo run -p opc-da-client-test`（= e2e 模式）→ 期望 exit 0。
- client-test e2e 自己 spawn server 子进程（SimDataSource + GeneratedDataSource 两个独立实例）。SCM 因子进程已 `CoRegisterClassObject`（REGCLS_MULTIPLEUSE）而复用它，不另启动。
- `/RegServer` 仍需：让 SCM 知 CLSID→exe，client 经 ProgID 连时 SCM 才能路由到 client-test spawn 的 server 子进程。

---

## 12. 分阶段

- **P4.1**（e2e 全流程）：
  - opc-da-server：env 切数据源 + `Factory` 持 `Arc<dyn DataSource>` 注入。
  - opc-da-client-test：模块化重构（`main` / `server_proc` / `e2e` / `report`）；e2e 模式（13 flat 迁移 + hierarchical 探针，见 §7）。
  - commit + verify（13 探针无回归 + hierarchical pass + exit 0）。
- **P4.2**（压测）：
  - `stress.rs`（M 并发 client + 指标采集 + 矩阵参数）；`server_proc` 加指标读取；`report` 加压测输出。
  - commit + 跑 v1 矩阵（100 client / 1w 组）验证达标。

---

## 13. 风险

- **R1（SCM vs spawn 路由竞态）**：client-test spawn server 后 client 连太快 → SCM 可能另启动一个 server 实例。缓解：`server_proc` 等就绪信号（stderr `serving`）再返回，client-test 此时才连。
- **R2（server.exe 路径）**：client-test 找 server.exe。env `OPC_DA_SERVER_EXE` 或默认 `target/debug/opc-da-server.exe`。release 构建需显式指定。
- **R3（Windows API 指标 feature 名）**：`GetProcessHandleCount` / `GetProcessMemoryInfo` 所属 windows feature 实现时查证（`Win32_System_Threading` / `Win32_System_ProcessStatus`）。
- **R4（COM 进程隔离）**：client-test（tokio multi-thread）与 server 子进程是独立 COM 进程。`OpcDaClient` worker 管 client 侧 COM；server 子进程独立 CoInit。无冲突。
- **R5（e2e 规模 vs 压测规模）**：e2e 用 2/2/3 小规模验 browse 正确性（快），10w browse 延迟由 stress v3 覆盖。

---

## 14. 验证标准

- **P4.1**：`cargo run -p opc-da-client-test`（e2e）→ 13 flat + hierarchical 全 pass，exit 0；`pwsh scripts/verify.ps1` 全门过（含 13 探针无回归）。
- **P4.2**：`cargo run -p opc-da-client-test stress --clients 10 --duration 10` → 输出指标，无 panic；v1 矩阵（100 client / 1w 组 / 60s）达标。

---

## 附录 A：与 scale-plan §P4 的差异

- scale-plan §P4 写"stress.rs 或独立 crate"→ 本设计改为改造 `opc-da-client-test`（单 binary + 子命令）。
- scale-plan §P4 "mock client 复用 opc-da-client subscribe"→ 保留（stress 模式每线程一个 `OpcDaClient`）。
- scale-plan §P4 指标"延迟 = OnDataChange ts vs DataSource read ts"→ 修正为推送间隔（client 拿不到 read ts）。
- P2.3 遗留的"opc-da-client-test browse 探针扩展（hierarchical）"→ 在本设计 P4.1 e2e hierarchical 探针中完成。
