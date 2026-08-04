# opc-da-server-sim

基于 [`opc-da-server`](../opc-da-server) 库的 **OPC DA Simulation Server** 示例——对标
`Matrikon.OPC.Simulation.1`，作为 workspace 中与 `opc-cli` / `opc-da-desktop` 对等的 server 侧示例，
完整示范「接入自定义 `DataSource` + 注册 + 运行」流程，便于下游照抄改造成自己的协议网关 server
（Modbus / S7 / UA 桥接等）。

- **Windows-only**（OPC DA 基于 COM/DCOM；非 Windows 由 `opc-da-server` 的 `compile_error!` 拒编译）。
- **薄包装**：复制库 bin 的 COM 编排骨架，仅替换 ProgID / CLSID / `DataSource`。

## 功能

| 维度 | 说明 |
|---|---|
| ProgID | `opc-da-rs.Sim.1`（Version-Independent `opc-da-rs.Sim`），独立 CLSID（与库 `opc-da-rs.Server.1` 不冲突）|
| tag 集 | 8 类型模板 × `count` + `_System.Time` 单例；默认 `count=100` → 801 tag |
| 命名空间 | hierarchical，按 `.` 自动建树（如 `Random.Int4.7`）|
| 值生成 | read-time 纯计算：random / square / sawtooth / triangle / altbool / systime；`BucketBrigade`/`WriteTag` 可写（write-store / read-load）|
| 规模化 | env `OPC_DA_SIM_COUNT`（默认 100，上限 100 000）→ 最高 ≈ 80 万 tag，支撑大规模订阅 |
| 数据质量 | 已知 item `OPC_QUALITY_GOOD`（0xC0）；未知 / 越界 index `OPC_QUALITY_BAD`（0x00）|

## 架构

```
opc-da-server-sim/
├── Cargo.toml          # name=opc-da-server-sim；deps: opc-da-server, opc-da-client, windows
└── src/
    ├── main.rs         # cfg(target_os=windows) 门控 + args(/RegServer /UnregServer) + main()
    ├── runtime.rs      # CLSID/ProgID 常量 + build_registration + run_register/unregister/run_server（COM 编排）
    ├── data_source.rs  # SimDataSource: DataSource trait 实现 + VARIANT helper + read/write
    ├── tags.rs         # TagType 表 + expand_ids(count) + build_namespace_tree（按 '.' Trie 建树）
    └── waveform.rs     # enum TagKind + value() 纯函数生成器
```

**依赖链**：`waveform`（值生成）→ `tags`（类型表 + 命名空间）→ `data_source`（trait 实现）→ `runtime`（COM 编排）→ `main`（装配）。

## 构建

从 workspace 根目录：

```bash
cargo build -p opc-da-server-sim
# 或 release：
cargo build -p opc-da-server-sim --release
```

## 注册（需管理员）

OPC DA server 是 out-of-process COM server，client 经 ProgID/CLSID 通过 SCM 激活，必须先注册：

```powershell
# 管理员终端
target\debug\opc-da-server-sim.exe /RegServer      # 写 HKCR（CLSID/ProgID/CATID/AppID，64+32 双视图）
target\debug\opc-da-server-sim.exe /UnregServer    # 清注册项（幂等）
```

注册后，标准 OPC client（Kepware / Matrikon / Graybox / Prosys / Takebishi / `opc-cli`）即可枚举到
`opc-da-rs OPC DA Simulation Server` 并 browse / read / write / subscribe。

## 运行

注册后手动启动 server（client 连接时 SCM 经 `REGCLS_MULTIPLEUSE` 复用已运行实例）：

```powershell
# 默认 count=100（801 tag）
target\debug\opc-da-server-sim.exe

# 大规模（80001 tag，用于压测订阅规模化）
$env:OPC_DA_SIM_COUNT = "10000"
target\debug\opc-da-server-sim.exe
```

启动后输出 `opc-da-server-sim: serving (ProgID=opc-da-rs.Sim.1, N tags, Ctrl+C 退出)` 并阻塞。
**Ctrl+C 终止**（库 `LockServer` 暂为 no-op，无优雅退出路径，见下「已知限制」）。

## tag 类型表

| item_id 模板 | dtype | 波形 | 可写 | EU range |
|---|---|---|---|---|
| `Random.Int4.{i}` | VT_I4 | random 0..=100 | 否 | (0, 100) |
| `Random.Real8.{i}` | VT_R8 | random 0.0..100.0 | 否 | (0, 100) |
| `Square.Real8.{i}` | VT_R8 | 方波 0/100 | 否 | (0, 100) |
| `Sawtooth.Real8.{i}` | VT_R8 | 锯齿 0..100 | 否 | (0, 100) |
| `Triangle.Real8.{i}` | VT_R8 | 三角 0..100..0 | 否 | (0, 100) |
| `BucketBrigade.Int4.{i}` | VT_I4 | 计数器（write 设值） | **是** | (0, 100) |
| `WriteTag.Int4.{i}` | VT_I4 | 寄存器（write 覆盖） | **是** | — |
| `AltBool.Bool.{i}` | VT_BOOL | 交替 true/false | 否 | — |
| `_System.Time`（单例，无 `.{i}`） | VT_R8 | UNIX epoch 秒 | 否 | — |

`{i}` = 0..count-1。browse 树形：`Random → Int4 → {0,1,...}`（数字索引叶）。

## 环境变量

| 变量 | 默认 | 范围 | 说明 |
|---|---|---|---|
| `OPC_DA_SIM_COUNT` | 100 | 1..=100 000 | 每类型实例数（上限防 OOM）|

## 测试

```bash
cargo test -p opc-da-server-sim    # 30 单测：waveform(6) + tags(5) + data_source(18) + runtime(1)
```

数据源正确性（值域 / read-write 往返 / 越界语义 / deadband EU）由单测覆盖。端到端互操作用标准
OPC client 手测（browse / read / write / subscribe）。

## 已知限制

- **Ctrl+C 退出**：库 `IClassFactory::LockServer` 暂为 no-op，`CoReleaseServerProcess()==0` 优雅退出未接线
  （库后续阶段实装）；目前靠控制台默认 Ctrl+C 终止。
- **SCM 自动启动不带 env**：若 client 触发 SCM 按 `LocalServer32` 自动启动（用户未手动开 server），
  `OPC_DA_SIM_COUNT` 不被继承 → count 落回默认 100。主场景（手动启动 + client 连）不受影响。
- **远程 DCOM 激活**：跨机连接需 `OPCproxy.dll` + server 以 Windows Service / RunAs 方式运行
  （console EXE 仅本机激活）。本机 e2e 不涉及。
- **`/RegServer` 需管理员**：写 `HKLM` / `HKCR`。

## 与 `opc-da-server` 库的关系

`opc-da-server` 是 OPC DA Custom Server **库**（lib + 自带一个最小 demo bin `opc-da-rs.Server.1`，
4 个 tag 的 `SimDataSource`）。本 crate 是基于该库的**独立示例**：

- 独立 ProgID / CLSID（不冲突）；
- 更完整的 Matrikon 风格 tag 集（8 类型 × count）；
- 示范自定义 `DataSource`（`tags.rs` 类型表 + `waveform.rs` 值生成 + `data_source.rs` trait 实现）的完整接入流程。

库的 `DataSource` trait（`opc-da-server/src/data_source.rs`）是扩展点——本 crate 的 `SimDataSource`
即一个参考实现，可作为编写协议网关数据源（Modbus / S7 / UA 桥接）的模板。

---

详见设计 spec：[`docs/superpowers/specs/2026-08-04-opc-da-server-sim-design.md`](../docs/superpowers/specs/2026-08-04-opc-da-server-sim-design.md)。
