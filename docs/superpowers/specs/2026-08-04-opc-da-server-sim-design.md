# opc-da-server-sim 示例 server 设计

> 日期：2026-08-04　分支：`feat/opc-da-server`
> 状态：设计定稿，待实现　真相源：本文件 + `opc-da-server/src/` + git log
> 上游库：`opc-da-server`（`docs/superpowers/specs/2026-08-02-opc-da-server-design.md`）
> 规模化背景：`docs/superpowers/specs/2026-08-03-opc-da-server-scale-plan.md`

---

## 0. 文档定位

`opc-da-server` 是一个 OPC DA Custom Server **库**（lib + 自带一个最小 demo bin `opc-da-rs.Server.1`）。和 `opc-da-client` 已有 `opc-cli` / `opc-da-desktop` 两个示例对等，server 侧也需要一个示例：**`opc-da-server-sim`**——基于库搭一个对标 `Matrikon.OPC.Simulation.1` 的模拟 server，完整示范「接入自定义 `DataSource` + 注册 + 运行」流程，便于下游照抄改造成自己的协议网关 server（Modbus/S7/UA 桥接等）。

**本文档是设计 spec**，不含逐步实现指令（那是 writing-plans 的产物）。

---

## 1. 目标与非目标

### 1.1 目标

| 目标 | 指标 / 描述 |
|---|---|
| 独立示例 crate | `opc-da-server-sim` 成为 workspace 第 6 个 member |
| 薄包装 | main/runtime 复制 `opc-da-server/src/bin/opc-da-server.rs` 的 COM 编排骨架，仅替换 ProgID/CLSID + DataSource |
| 可规模化 tag 集 | 8 种类型模板 × `count`（默认 100）+ 1 个单例 `_System.Time`；默认 801 tag |
| count 可调 | 环境变量 `OPC_DA_SIM_COUNT`（默认 100，上限 10 万）→ 最高 ≈ 80 万 tag |
| 大规模订阅 | 支撑多 client × 全 tag 并发订阅（库 scheduler 已验证 100w 订阅） |
| 可写 tag | 演示 Write：`BucketBrigade.Int4`（计数器）+ `WriteTag.Int4`（寄存器） |

### 1.2 非目标（YAGNI）

- in-proc DLL（库只支持 out-of-proc EXE）
- 1:1 复刻 Matrikon 全集（几十~上百 tag 全类型排列）
- 配置文件 / clap CLI（保持简洁，只 env）
- 优雅退出（库 `LockServer` no-op 限制，见 §10）
- 改动 `opc-da-server` 库自带 bin（`opc-da-rs.Server.1` 保留为最小 demo）
- 远程 DCOM 激活（需 OPCproxy.dll + Service/RunAs，README 注明即可）

---

## 2. 已确认的需求决策

| # | 决策 | 选择 |
|---|---|---|
| 1 | 定位 | 独立 crate，薄包装，纯 bin（仿 `opc-cli`） |
| 2 | tag 命名 | `Random.Int4.7`，按 `.` 自动建 hierarchical 树 |
| 3 | 类型集 | 8 种参与 count 展开 + `_System.Time` 单例（共 9 模板） |
| 4 | count 配置 | 环境变量 `OPC_DA_SIM_COUNT`（默认 100，1..=100_000） |
| 5 | ProgID | `opc-da-rs.Sim.1` / Version-Independent `opc-da-rs.Sim` |
| 6 | CLSID | 新生成 GUID（与库 `CLSID_OPC_DA_SERVER` 不同） |
| 7 | 验收 | 质量门 + 自闭环 e2e + 单测；标准 client 互操作可选人工抽查 |
| 8 | 订阅 | 支持大规模订阅（性能），`DataSource::read` 纯计算无 IO |

---

## 3. crate 结构（纯 bin，仿 `opc-cli/Cargo.toml`）

```
opc-da-server-sim/
├── Cargo.toml          # name = "opc-da-server-sim"；path deps: opc-da-server
├── README.md           # 仿 opc-da-desktop/README.md 范式
└── src/
    ├── main.rs         # cfg(target_os="windows") 门控 + args(/RegServer /UnregServer) + run()
    ├── runtime.rs      # run(): COM 编排（复制 bin 模板）+ build_registration() + CLSID/ProgID 常量
    ├── data_source.rs  # SimDataSource: DataSource trait 实现
    ├── tags.rs         # TagType 表 + count 展开 + build_namespace_tree()（按 '.' 建 NsNode）
    └── waveform.rs     # enum Waveform + 纯函数生成器 + #[cfg(test)] 单测
```

**`Cargo.toml` 要点**：
- `[package]` 继承 workspace（`edition = "2024"`、`rust-version = "1.93.1"`）。
- `[dependencies]`：`opc-da-server = { path = "../opc-da-server" }` + `windows`（`Win32_Foundation` / `Win32_System_Com` / `Win32_System_Variant`）。**不引入 clap/toml/anyhow**。错误处理用 `eprintln!` + `std::process::exit(1)`（与 bin 风格一致），保持 0 新增非 workspace 依赖。
- 工作区根 `Cargo.toml` 的 `members` 加 `"opc-da-server-sim"`。
- 不加 `[features]`（库无 feature）。

**非 Windows 门控**：`main.rs` 非 Windows 编译出友好 `compile_error!("opc-da-server-sim requires Windows")`，与库 `lib.rs:35-38` 一致。

---

## 4. DataSource 设计（核心教学点）

### 4.1 类型表（声明式）

8 种参与 count 展开 + 1 单例。item_id 规则：展开类型 = `{prefix}.{i}`（i = 0..count）；单例 = prefix 本身。

| prefix | dtype | Waveform | writable | EU range | 展开 |
|---|---|---|---|---|---|
| `Random.Int4` | `VT_I4` | Random | 否 | `(0,100)` | ×count |
| `Random.Real8` | `VT_R8` | Random | 否 | `(0,100)` | ×count |
| `Square.Real8` | `VT_R8` | Square | 否 | `(0,100)` | ×count |
| `Sawtooth.Real8` | `VT_R8` | Sawtooth | 否 | `(0,100)` | ×count |
| `Triangle.Real8` | `VT_R8` | Triangle | 否 | `(0,100)` | ×count |
| `BucketBrigade.Int4` | `VT_I4` | Counter | **是** | `(0,100)` | ×count |
| `WriteTag.Int4` | `VT_I4` | Register | **是** | `None` | ×count |
| `AltBool.Bool` | `VT_BOOL` | AltBool | 否 | `None` | ×count |
| `_System.Time` | `VT_R8` | SysTime | 否 | `None` | 单例 |

骨架（`tags.rs`）：
```rust
pub(crate) struct TagType {
    pub prefix: &'static str,
    pub dtype: VARENUM,
    pub wf: Waveform,
    pub writable: bool,
    pub range: Option<(f64, f64)>,
    pub singleton: bool,   // true = 不参与 count 展开（_System.Time）
}
pub(crate) const TYPES: &[TagType] = &[ /* 上表 9 行 */ ];
```

### 4.2 命名空间构建（按 `.` 自动建树）

库 `NamespaceTree` 无"按 `.` 自动建树"API（`data_source.rs:55-113`：仅 `new(leaves)` flat / `from_tree(root)` 手建）。sim 实现一个 Trie 式 helper：

```rust
/// 按 '.' 分割 ids 建 hierarchical NsNode 树（合并公共前缀）。
fn build_namespace_tree(ids: &[String]) -> NsNode { /* Trie */ }
```

`Random.Int4.0..Random.Int4.{count-1}` → 树形：`Random → Int4 → {0,1,...}`（数字索引作为 Leaf 节点名）。`_System.Time` → `_System → Time`（叶）。

`SimDataSource::new(count)` 流程：
1. 遍历 `TYPES`：非 singleton 的生成 count 个 `"{prefix}.{i}"`；singleton 的加 prefix 本身。
2. 收集全部 item_id → `build_namespace_tree` → `NamespaceTree::from_tree(root)`。
3. 同时建 `HashSet<String>`（O(1) 存在性判断，仿 `GeneratedDataSource` 的 `leaves`）。
4. 为两个可写类型预分配 `Vec<AtomicI32>`（各 count 长，初值 0）。

### 4.3 read / write 分派

`read(item_id)`：
1. 若 `_System.Time` → 返回当前 `SystemTime` epoch 秒（f64）。
2. 否则解析 `item_id`：在 `TYPES` 中线性查找 `prefix` 满足 `item_id.starts_with(prefix) && item_id.as_bytes()[prefix.len()] == b'.'`，取剩余部分 `parse::<usize>()` 为 index；失败 → 返回空 VARIANT + `OPC_QUALITY_BAD`。
3. 按 `TagType.wf` 分派到 `waveform::value(wf, index, elapsed, &regs)` 纯函数。

`write(item_id, value)`：
1. 解析类型 + index（同上）。
2. 若 `TagType.writable == false` → `E_ACCESSDENIED`。
3. `variant_as_i4(value)` 失败 → `E_INVALIDARG`。
4. 写对应 `Vec<AtomicI32>[index]`（Counter / Register 均为 write-store / read-load，与库 `SimDataSource.BucketBrigade` 一致；两者仅命名对标 Matrikon 分类，行为相同）→ `S_OK`。

`item_meta(item_id)` → `Some(ItemMeta{ data_type, writable })`（按类型）；未知 `None`。
`item_range(item_id)` → 类型表 `range`。
`query_organization()` → `NsOrganization::Hierarchical`。

---

## 5. 值生成器（read-time 纯计算，无后台 task）

全部在 `waveform.rs`，确定性（同 index + 同时刻复现），与库 `SimDataSource` read-time 模式一致（`data_source.rs:8-11`）。`elapsed` = `Instant::now() - start`，`index` = tag 实例序号（用于相位错开）。

| Waveform | 公式 | 值域 |
|---|---|---|
| Random (Int4) | `((index as u64).wrapping_mul(2_654_435_761).wrapping_add(elapsed_secs)) % 101` | 0..=100 (i32) |
| Random (Real8) | 同上种子 `% 10001 / 100.0` | 0.0..=100.0 (f64) |
| Square | `if (elapsed_secs + index as u64) % 2 == 0 { 100.0 } else { 0.0 }` | 0/100 (f64) |
| Sawtooth | `((elapsed_secs + index as u64) % 10) as f64 / 10.0 * 100.0` | 0..100 锯齿 (f64) |
| Triangle | `let p=(elapsed_secs+index as u64)%10; let t=if p<5 {p} else {10-p}; t as f64/5.0*100.0` | 0..100..0 三角 (f64) |
| AltBool | `(elapsed_secs + index as u64) % 2 == 0` | bool |
| Counter | `Vec<AtomicI32>[index].load(Relaxed)` | i32（write 设值持久化） |
| Register | `Vec<AtomicI32>[index].load(Relaxed)` | i32（write 覆盖；行为同 Counter） |
| SysTime | `SystemTime::now().duration_since(UNIX_EPOCH).as_secs_f64()` | f64 秒 |

`elapsed_secs` = `elapsed.as_secs()`（每秒变一次，与库 `random_i4`/`square_wave` 节奏一致）。

**纯计算无 IO**：满足 `scheduler` 锁内调 `DataSource::read` 的约束（`objects/scheduler.rs:273-300` 注释），不阻塞 worker，支撑大规模订阅。

---

## 6. 注册 / 运行 / 退出

### 6.1 注册参数（`runtime.rs::build_registration`）

```rust
// 固定示例常量（与库 CLSID_OPC_DA_SERVER 0x9a7b_3c2d_... 不同）；如担心碰撞可换 uuidgen 产物，非必需。
const CLSID_OPC_DA_SIM: GUID = GUID::from_u128(0xb1c2_d3e4_f5a6_0718_293a_4b5c_5d6e_7f80);
const PROG_ID: &str = "opc-da-rs.Sim.1";
const VIPROG_ID: &str = "opc-da-rs.Sim";
const DESCRIPTION: &str = "opc-da-rs OPC DA Simulation Server";
```

`build_registration()` 构造 `ServerRegistration { clsid, prog_id, version_independent_prog_id, exe_path: current_exe(), catids: &[CATID_OPCDAServer10/20/30], app_id: clsid, description }`（仿 `bin/opc-da-server.rs:32-40`）。CATID 用 `opc_da_client::bindings::da` 的常量。

### 6.2 run() 编排（复制 bin 模板）

```
parse args: /RegServer → register(&reg) return;  /UnregServer → unregister(&reg) return
read count = env OPC_DA_SIM_COUNT (默认 100, clamp 1..=100_000)
CoIncrementMTAUsage()
scheduler::init(available_parallelism)        // 必须在 group 创建前
CoInitializeSecurity(None, RPC_C_AUTHN_LEVEL_CONNECT, RPC_C_IMP_LEVEL_IDENTIFY, None, EOAC_NONE)
let ds = Arc::new(SimDataSource::new(count))
let factory: IClassFactory = Factory::new(ds).into()
CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED)
CoResumeClassObjects()
loop { sleep(1s) }                             // 不接 Ctrl+C handler，依赖控制台默认终止（库 LockServer no-op，见 §10）
```

顺序与约束严格照搬 `opc-da-server/src/bin/opc-da-server.rs:83-120`。

### 6.3 args 解析

手写（不引入 clap）：`argv[1]` 匹配 `/RegServer` / `/UnregServer`（大小写不敏感）；其余忽略。`-count` **不**走 args（用 env，见 §7）。

---

## 7. 配置（环境变量）

```rust
fn read_count() -> usize {
    env::var("OPC_DA_SIM_COUNT")
        .ok().and_then(|s| s.parse().ok())
        .filter(|&n| (1..=100_000).contains(&n))
        .unwrap_or(100)
}
```

- **默认 100**：8 类型 × 100 + 1 = 801 tag。
- **上限 10 万**：避免误填导致 OOM（namespace + 可写寄存器 Vec）。
- 用法：`set OPC_DA_SIM_COUNT=10000` → 80_001 tag。
- **与库 env 约定同族**（`OPC_DA_DATASOURCE` / `OPC_DA_GEN_PLANTS`），零学习成本。
- **已知局限（README 注明）**：仅"手动启动 server + client 连"可靠生效（`REGCLS_MULTIPLEUSE` 复用已运行实例）。若 client 触发 SCM 自动启动（按 `LocalServer32` 裸启动，不带 env），count 落回默认 100。需要 SCM 自动启动也带 count 是后续增强（注册时写 `LocalServer32` 命令行），现不做。

---

## 8. 约束遵守（质量门硬性）

- **Windows-only**：非 Windows `compile_error!`。
- **禁止 panic**（`CLAUDE.md:125`）：mutex 用 `locked()` helper（`unwrap_or_else(PoisonError::into_inner)`）；`usize→u32` 用 `try_from().unwrap_or(u32::MAX)`；env parse 用 `unwrap_or`。
- **unsafe**：主要在 `runtime.rs` 的 COM 调用（`CoIncrementMTAUsage` / `CoInitializeSecurity` / `CoRegisterClassObject`），每个 unsafe 块带 `// SAFETY:` 注释（复制 bin 的注释为模板）。
- **MTA 线程模型**不破坏：不引入自己的 COM 线程；`SimDataSource: Send + Sync`（`AtomicI32` + 只读 `NamespaceTree`）。
- **`DataSource::read` 纯计算无 IO**（§5）。
- **clippy**：`all = deny`、`pedantic/cargo/nursery = warn`、`undocumented_unsafe_blocks = deny`（workspace 继承）。

---

## 9. 验收 / 测试

### 9.1 质量门（必须全过）
`cargo build -p opc-da-server-sim` + `make verify`（`cargo fmt --check` → `cargo clippy --workspace --all-targets --all-features -D warnings` → `cargo test --doc` → `cargo test --workspace` → compat 逐个构建）。

### 9.2 单测（`waveform.rs` / `tags.rs` 内联 `#[cfg(test)]`）
- **Waveform 确定性**：同 `(wf, index, elapsed_secs)` 必出同值（random/square/sawtooth/triangle/altbool 各一组）。
- **值域**：Random ∈ 0..=100、Square ∈ {0,100}、Sawtooth/Triangle ∈ 0..=100。
- **相位错开**：同 elapsed 不同 index 出不同值（至少存在一组）。
- **count 展开**：`new(5)` 生成 8×5+1=41 个 item_id，无重复，`_System.Time` 恰 1 个。
- **namespace 树**：`browse_children(&["Random"])` 含 `Int4`/`Real8` 分支；`browse_children(&["Random","Int4"])` 含 5 个叶。
- **read/write 往返**：`write("BucketBrigade.Int4.3", 42)` → `read` 得 42；只读 tag `write` 返 `E_ACCESSDENIED`；类型不符返 `E_INVALIDARG`；未知 item `read` 返 `OPC_QUALITY_BAD`。

### 9.3 自闭环 e2e（改 `opc-da-client-test`）
小改 `opc-da-client-test` 支持 ProgID 覆盖（env `OPC_DA_SERVER_PROGID`，默认 `opc-da-rs.Server.1`）：
- `ServerChild::spawn` 同时设 `OPC_DA_DATASOURCE` + 新增 `OPC_DA_SERVER_PROGID`。
- `e2e.rs` / `stress.rs` 的 `const PROG_ID` 改为读 env。
- 跑 `OPC_DA_SERVER_PROGID=opc-da-rs.Sim.1 cargo run -p opc-da-client-test` → browse/read/write/subscribe 全 tag 通过。
- stress 跑 sim 验证规模化：`OPC_DA_SIM_COUNT=10000` + 多 client × 全 tag 订阅 60s，无报错、RSS 稳态。

### 9.4 标准 client 互操作（可选，人工抽查，不阻塞完成）
Kepware / Matrikon / Graybox 等 GUI client 枚举到 `opc-da-rs OPC DA Simulation Server`，browse/read/write/subscribe 正常。

---

## 10. 风险 / 已知坑

| # | 风险 | 应对 |
|---|---|---|
| 1 | `/RegServer` 需管理员权限 | README 注明；`scripts/` 可加提权 helper（后续） |
| 2 | SCM 自动启动不带 env → count 落回默认 | §7 已述；主场景（手动启动）不受影响 |
| 3 | `LockServer` no-op（`class_factory.rs:53-58`）→ 无法优雅退出 | 保持 Ctrl+C；README 注明为已知限制，不在此修库 |
| 4 | CLSID 双视图写 | 库 `registry::register` 已处理（`KEY_WOW64_64KEY \| KEY_WOW64_32KEY`），照搬即可 |
| 5 | 远程 DCOM 激活需 OPCproxy.dll + Service/RunAs | README 注明；本机 e2e 不涉及 |
| 6 | `opc-da-client-test` ProgID 改动要进质量门回归 | §9.3 的改动作为本任务一部分，同 PR 提交 |
| 7 | count 上限 10w → namespace + 2 个 Vec<AtomicI32> 内存 | 10w × (item_id String ~20B + 树节点) ≈ 数十 MB，可接受；超出 clamp |

---

## 11. 落地里程碑（粗，供 writing-plans 展开）

1. **crate 骨架**：`Cargo.toml` + workspace 注册 + `main.rs`（非 Windows 门控） + 空 `run()`。→ 验证：`cargo build -p opc-da-server-sim` 过。
2. **waveform + tags**：`waveform.rs`（9 生成器 + 单测）+ `tags.rs`（类型表 + count 展开 + build_namespace_tree + 单测）。→ 验证：单测全过。
3. **SimDataSource**：`data_source.rs` 实现 `DataSource` trait + read/write 往返单测。→ 验证：单测全过。
4. **runtime + 注册**：`runtime.rs`（CLSID/ProgID + build_registration + run 编排，复制 bin）。→ 验证：`/RegServer` 写注册表（双视图）、`/UnregServer` 清理、裸启动 `CoResumeClassObjects` 不报错。
5. **client-test ProgID 覆盖 + e2e**：改 `opc-da-client-test`，跑通 sim 全 tag 的 browse/read/write/subscribe + stress。→ 验证：e2e + stress 通过。
6. **README + 质量门**：仿 `opc-da-desktop` 写 README；`make verify` 全过。→ 验证：verify 0 退出。

每步独立 commit + 验证后再下一步。
