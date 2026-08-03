# opc-da-server 自主循环 Runbook

> 用途：维护者外出期间，由 Claude Code `/loop`（dynamic）无人值守推进 opc-da-server。
> 分支：`feat/opc-da-server`。真相源：本 Runbook（checklist）+ 设计文档（§5–§14）+ git log。
> 终极真相：`docs/superpowers/specs/2026-08-02-opc-da-server-design.md`。

---

## 0. 前提与硬约束（循环前必读）

- **会话/电脑需持续运行**：`/loop` 仅在 REPL idle 时 fire；关机/关终端/睡眠 → 循环停。
- **单会话推进**：同一时刻只有一个会话跑这个 loop（避免 git 冲突）。
- **每轮必过 `verify.ps1` 才 commit**：绝 `git add` 未过质量门的代码。失败 → 见 §4。
- **不改 opc-da-client 逻辑**：复用其公开资产（`bindings`/`com_utils`/`typedefs`）；发现 client 真 bug 必须修（BUG 零容忍），但为 server 改 client 禁止。
- **每个 `unsafe` 块写 `// SAFETY:` 注释**（项目约定，clippy `undocumented_unsafe_blocks=deny`）。

## 1. 能自主 vs 需用户

**能自主**（循环推进）：阶段 1（Group/IO/订阅/SimDataSource）、阶段 2（Browse/DA3）、阶段 3 **代码**（CoInitializeSecurity/Service/unregister）、阶段 4 硬化（unsafe/并发/e2e 入 verify）。

**需用户回**（**遇即停**，写 STATUS）：DCOM 远程验证、真实 OPC client 互操作、管理员操作（`/RegServer` 实跑）、CLSID 正式分配、crates.io 发版。

## 2. 每轮精确步骤（循环每轮照做）

1. `git pull origin feat/opc-da-server`（同步；冲突 → 停）。
2. 读本 Runbook 的 §5 checklist + `git log --oneline -10`（上次到哪）。
3. 选 §5 第一个未勾选 `[ ]` 且属"能自主"的项。**若下一个是"需用户"项 → 跳到 §4 停止。**
4. 按设计文档对应章节实现（§5 Server / §6 Group / §8 ConnectionPoint / §9 DataSource / §10 推送 / §11 注册）。
5. `cargo build -p opc-da-server`。
6. `cargo test -p opc-da-server`（必须含同进程 COM 激活测试，参考 `class_factory::tests::self_activate_via_coregister` 模式）。
7. `pwsh -File scripts/verify.ps1`（全门：fmt + clippy workspace + doc test + test + compat）。**硬门槛。**
8. 通过 → `git add -A` + `git commit -m "<conventional>"` + `git push origin feat/opc-da-server` + `git push github feat/opc-da-server`。
9. 勾选本 Runbook checklist（`[ ]` → `[x]`）+ `git add` Runbook + `git commit -m "chore(runbook): 勾选 <task>"` + push 两 remote。
10. 判断：§5 还有"能自主" `[ ]`？有 → 调度下一轮（ScheduleWakeup）；无 → §4 停止（完成报告）。

**端到端验证（计划 `purring-chasing-dusk.md`）**：每轮 server 接口实装 + verify + commit 后，在 `opc-da-client-test` 加对应命令，`opc-da-server /RegServer` + 跑端到端实测 pass。改 server 后须先 `taskkill /F /IM opc-da-server.exe`（释放 exe 锁）才能重 build；Git Bash 调 `/RegServer` 用 pwsh 绕过 MSYS2 路径转换。

## 3. 关键技术约定（避免重复踩坑，详见设计文档 §18）

- 多接口 `#[implement(A, B)]` → `impl A_Impl for XXX_Impl`（宏生成的 `_Impl` 类型，非原始）。
- windows 0.61：`BOOL`/`HRESULT`/`GUID` 在 `windows::core`（非 `Win32::Foundation`）；`E_NOTIMPL`/`E_OUTOFMEMORY`/`FILETIME` 在 `Win32::Foundation`。
- `CoRegisterClassObject` 4 参返回 `Result<u32>`（cookie 在返回值）。
- `IClassFactory_Impl::CreateInstance` 第二参 `Ref<IUnknown>`；`Interface::query(&self, iid, ppv) -> HRESULT`。
- Registry API 返回 `WIN32_ERROR`（`.ok()` 转 Result）；`&raw mut hkey` 替代 `&mut`（borrow_as_ptr）；指针 cast 用 `.cast::<T>()`（ptr_as_ptr）。
- `#[implement]` 宏触发 lint：文件级 `#![allow(clippy::ref_as_ptr, clippy::inline_always, clippy::undocumented_unsafe_blocks, clippy::not_unsafe_ptr_arg_deref)]`（对齐 `opc-da-client/subscription.rs`）。
- CATID_OPCDAServer20/30 值已核实正确（`...642`），勿改。

## 4. 失败 / 停止处理

**编译或 verify 失败**：最多尝试修复 3 次（每次查 windows-rs 0.61 签名 / bindings / 设计文档）。仍失败 → `git restore`（绝不 commit 坏代码）→ 写 STATUS → **停止循环**。

**卡住**（unsafe 坑 / 签名折腾 > 3 次 / 设计不清）→ 写 STATUS 记卡点 → 停。

**遇"需用户"项**（§1）→ 写 STATUS（"等待用户：<项>"）→ 停。

**停止时**：写 `docs/superpowers/specs/LOOP_STATUS.md`（当前 checklist 完成度 + 卡点/原因 + 下次续点 + 已 push 的最后 commit hash）→ commit + push 两 remote → `ScheduleWakeup(stop: true)`。

## 5. Checklist（进度跟踪，循环维护）

### 阶段 0（已完成，2026-08-02）
- [x] client 可见性暴露
- [x] crate 骨架 + workspace
- [x] 多接口 `#[implement]` spike
- [x] IClassFactory + CoRegisterClassObject 自激活
- [x] EXE 主循环 + `/RegServer` 分支
- [x] 注册工具 registry.rs
- [x] IOPCServer::GetStatus + IOPCCommon stub
- [x] verify.ps1 全门过 + commit + push

### 阶段 1 — MVP（client read/write/subscribe 自建 server）
- [x] `objects/connection_point.rs`：通用 `ConnectionPoint`（Advise/Unadvise/EnumConnections，持有 client sink 表）
- [x] `objects/group.rs`：Group 对象骨架 `#[implement(IOPCItemMgt, IOPCGroupStateMgt, IOPCSyncIO, IOPCAsyncIO2, IConnectionPointContainer)]`（注意 `_Impl` target）
- [x] `data_source.rs`：`DataSource` trait + `SimDataSource`（tag 树 + Random/Counter 值产生器；后台刷新用 read-time 计算，独立 task 留 §10 publisher）
- [x] `IOPCItemMgt`：AddItems / RemoveItems / ValidateItems / SetActiveState / SetClientHandles
- [x] `IOPCGroupStateMgt`（`c22b8ef` M4；GetState/SetState/SetName 实装；CloneGroup 暂 nyi）
- [x] `IOPCSyncIO`（`46672bf` M3；Read/Write，端到端 round-trip=42）
- [ ] `IOPCAsyncIO2`：Read / Write / Refresh2（CancelID + 走 callback）——骨架 nyi，待 DA3
- [x] `IOPCServer`（`b0ae27f` M2；AddGroup/RemoveGroup/GetStatus；GetGroupByName/CreateGroupEnumerator 暂 nyi）
- [x] `publisher.rs`（`51b1b83` M5b；周期推送 OnDataChange，跨进程 callback 端到端通）
- [x] **阶段 1 自闭环 e2e 测试**（opc-da-client-test 13 探针全 pass）

### 阶段 2 — Browse + DA3
- [x] `IOPCBrowseServerAddressSpace`（`ff9132c` M6；QueryOrganization=FLAT + BrowseOPCItemIDs）
- [x] `IOPCItemProperties`（`956b4f6` M7a；QueryAvailableProperties/GetItemProperties/LookupItemIDs）
- [ ] `IOPCGroupStateMgt2`（SetKeepAlive / GetKeepAlive）
- [ ] `IOPCSyncIO2`（ReadMaxAge / WriteVQT）
- [ ] `IOPCAsyncIO3`

### 阶段 3 — 代码部分（远程验证属"需用户"）
- [x] `CoInitializeSecurity` 实装（CONNECT/IDENTIFY/EOAC_NONE，注册类对象前调；本机 e2e 兼容 13 passed）
- [ ] Windows Service 包装（`windows-service` crate 或自写 ServiceMain）
- [x] `unregister` 完整实装（`RegDeleteTreeW` 递归删 CLSID/{prog_id}/AppID 子树，双视图 64+32 幂等）
- [ ] proxy/stub 配置文档（说明依赖 OPC Core Components，非编码）
- [ ] **→ 停止**：DCOM 远程真实验证（需用户配 DCOM + 第三方 client 跨机）

### 阶段 4 — 硬化
- [ ] unsafe / 内存 review（每个 unsafe 块 SAFETY 注释完备）
- [ ] 并发同步（`Mutex` 守护 group/item 注册表 + sink 表）
- [x] 阶段 1 自闭环 e2e 入 `verify.ps1`（End-to-End Gate：taskkill + build server + /RegServer + cargo run opc-da-client-test；固化 13 探针，每 verify 自动跑）

### 端到端 milestone（opc-da-client-test，计划 `purring-chasing-dusk.md`）
- [x] M1: 骨架 + `get_server_status` 端到端通（`375a388`；ServerObj 补 IConnectionPointContainer/IOPCItemProperties stub 满足 client v2::Server 强制 cast 4 接口）
- [x] M2: `IOPCServer::AddGroup/RemoveGroup`（`b0ae27f`；端到端 read 探针验证 AddGroup+AddItems 通，Read 待 M3）
- [x] M3: `IOPCSyncIO::Read/Write`（`46672bf`；端到端 read Random.Int4 + write Bucket Brigade=42 + round-trip=42 全 pass）
- [x] M4: `IOPCGroupStateMgt`（`c22b8ef`；GetState/SetState/SetName 实装 + 白盒 round-trip；set_subscription_rate 端到端待 M5 依赖 subscribe）
- [x] M5a: `FindConnectionPoint`（`f22d93f`；Group data_cp + Server shutdown_cp 接入；subscribe advise 端到端 cookie=1）
- [x] M5b: publisher 推送引擎（`51b1b83`；std::thread 周期推送 OnDataChange；subscribe 端到端收帧 5 passed）
- [x] M6: `IOPCBrowseServerAddressSpace`（`ff9132c`；QueryOrganization=FLAT + BrowseOPCItemIDs；browse 端到端列 4 tag，6 passed）
- [x] M7a: `IOPCItemProperties`（`956b4f6`；QueryAvailableProperties/GetItemProperties/LookupItemIDs；get_item_properties 端到端 4 property，7 passed）
- [x] M7b: `IOPCCommon` GetErrorString + `list_servers`（`bbb4c67`；双视图注册解决位宽坑；list_servers 枚举到 opc-da-rs，9 passed）
- [x] M8: 端到端全量验证（`5f3e53b`；13 探针全 pass——server 全部已实装接口覆盖；read_max_age/write_vqt/set_keep_alive 待 DA3 阶段 2）

### 需用户回（循环遇即停）
- 阶段 3 DCOM 远程验证 / 真实 client 互操作 / `/RegServer` 实跑 / CLSID 正式分配 / crates.io 发版

---

## 6. 新会话启动指令（用户）

在新会话（已 `cd E:\github.com\opc-cli`、分支 `feat/opc-da-server`）输入：

```
/loop
每轮按 docs/superpowers/specs/LOOP_RUNBOOK_opc-da-server.md §2 推进 opc-da-server 一个未勾选 task。
读 RUNBOOK checklist + git log 选下一项 → 实现 → cargo build/test + pwsh verify.ps1 → commit + push origin & github → 勾选 checklist + commit RUNBOOK。
遇"需用户"项或 verify 修不了（3 次）→ 写 LOOP_STATUS.md + commit/push + 停止循环。
绝不 commit 未过 verify 的代码。
```

（`/loop` 进入 dynamic 自调度；Claude 用 ScheduleWakeup 安排下一轮。电脑需保持运行。）
