# opc-da-server 技术实现方案

> 状态：方案（未实现）。基于 2026-08-02 四路深度调研（OPC DA Server 规范 / windows-rs COM server 能力 / 现有代码复用度 / 开源参考与测试闭环）。
> 日期：2026-08-02。
> 前置决策（已与维护者确认）：**复用路径 = 可见性暴露**——opc-da-client 把 `bindings` / `opc_da::com_utils` / `opc_da::typedefs` 等从私有改为公开（`mod` → `pub mod`，**零逻辑/行为改动**），opc-da-server 依赖 opc-da-client 最大化复用。
> 硬约束：**不得为实现 server 而修改 opc-da-client 的逻辑**；发现 client 真 bug 则必须修（BUG 零容忍）。

---

## 1. 目标与非目标

### 目标
- 新增 crate **`opc-da-server`**：实现 OPC DA Custom Server（被 client 连接、暴露数据），即把现有 client「消费 COM 接口」反过来变成「实现 COM 接口 + 当 EXE 服务器 + 反向回调发起方 + 注册到注册表」。
- 复用 opc-da-client 的冻结 bindings / 内存工具 / 类型定义，省掉最危险的 ABI/vtable 手写层。
- **测试闭环**：用本项目 opc-cli / opc-da-desktop 作为 client 连自建 server（替代闭源 Matrikon 依赖），并经第三方 client（Matrikon Explorer / KEPServer Explorer）做互操作金标准护栏。

### 非目标（本方案不做）
- OPC UA（仅 DA；UA 是另一套无 COM 的协议）。
- OPC AE / HDA server（仅 DA）。
- XML-DA server。
- 生产级协议网关驱动（Modbus/S7 桥接）——架构留 `DataSource` 抽象口子，但不在本方案实现具体驱动。

---

## 2. 总体架构

### 2.1 crate 拓扑

```
            opc-da-client
          (pub mod bindings)        ← 可见性暴露（零逻辑改动）
          (pub mod opc_da::com_utils)
          (pub mod opc_da::typedefs)
                ▲
                │ depends
                │
          opc-da-server              ← 本方案新增
          ├── lib.rs    (嵌入 API: 启停 server、注册/卸载)
          ├── class_factory.rs  (IClassFactory + CoRegisterClassObject 生命周期)
          ├── registry.rs       (写 CLSID/ProgID/CATID/AppID)
          ├── objects/
          │   ├── server.rs     (Server 对象: IOPCServer/IOPCCommon/...)
          │   ├── group.rs      (Group 对象: IOPCItemMgt/IOPCSyncIO/...)
          │   └── connection_point.rs  (通用 IConnectionPoint<DIID>)
          ├── data_source.rs    (trait DataSource + SimDataSource)
          ├── publisher.rs      (订阅推送引擎: 打包并行数组→调 sink)
          └── bin/opc-da-server.rs   (LocalServer EXE 入口 + 主循环)
```

### 2.2 workspace 改动
- 根 `Cargo.toml`：`members` 加 `opc-da-server`。
- `opc-da-server/Cargo.toml`：`dependes on opc-da-client (path) + windows (workspace) + tokio (workspace) + thiserror/anyhow/tracing`。
- `#![allow(unsafe_code)]`（COM server 必须 unsafe；与 opc-da-client 同级，每个 unsafe 块写 `// SAFETY:`）。
- 双产物：`lib`（嵌入 API）+ `bin`（`opc-da-server.exe`，LocalServer）。

### 2.3 依赖方向（关键）
- opc-da-server → opc-da-client（复用 bindings / com_utils / typedefs / helpers）。
- opc-da-server **不**碰 client 的 `backend/` / `com_worker.rs` / `subscription.rs`（那是 client 的连接池/重连/sink 实现，与 server 方向相反，不通用）。

---

## 3. 复用映射（client 资产 → server 用法）

| client 资产 | 复用度 | server 用法 |
|---|---|---|
| `bindings/da`（IOPCServer / IOPCItemMgt / IOPCSyncIO / IOPCBrowseServerAddressSpace / IOPCItemProperties / IEnumOPCItemAttributes 等 23 接口的 `_Impl` trait + vtable） | 🟢 直接 | `#[implement(IOPCServer, …)]` + `impl IOPCServer_Impl for ServerObj` |
| `bindings/da`（IOPCDataCallback / IOPCShutdown 的 client 调用 `unsafe fn`） | 🟢 直接 | server 持 client sink 指针，调 `sink.OnDataChange(...)` / `sink.ShutdownRequest(...)` |
| `bindings/da`（CATID_OPCDAServer10/20/30 常量） | 🟢 直接 | 注册 Implemented Categories（已核实值正确：`CC603642-...`） |
| `opc_da/com_utils.rs`（`RemoteArray` / `LocalPointer` / VARIANT/SafeArray/BSTR 工具 / 转换 trait） | 🟢 大部分 | server 构造响应（OPCSERVERSTATUS / OPCITEMSTATE[] / 并行回调数组）直接用 |
| `opc_da/typedefs.rs`（ServerStatus / GroupState / ItemDef / ItemResult / 枚举 `to_native`） | 🟢 大部分 | server 序列化响应用 `to_native`；入参解析补 `from_native`（部分要新增，但加在 server 侧，不改 client） |
| `helpers.rs`（`opc_value_to_variant` / HRESULT 表 / FILETIME） | 🟡 中 | server 构造读响应 VARIANT + 错误码翻译 |
| `opc_da/client/traits/`（client 调用封装） | 🟡 参考 | 每个方法的签名/参数顺序对照表（省去重读 IDL）；方向相反，不搬代码 |
| `subscription.rs`（client 的 `#[implement(IOPCDataCallback)]` sink） | 🟡 参考 | 验证 `#[implement]` + frozen bindings 组合可用；展示并行数组**解析**；server 写反向**打包** |
| `backend/` + `com_worker.rs` | 🔴 不复用 | client 专用连接池/worker，与 server 无关 |

> 全新部分（client 不覆盖，server 自建）：`IClassFactory`、`CoRegisterClassObject` 生命周期、注册表写入、Server/Group 业务状态机、`DataSource`、订阅推送引擎、`IConnectionPointContainer`/`IConnectionPoint`（这俩来自 `windows-rs Win32::System::Com`，不在 OPC bindings）。

---

## 4. client 可见性暴露清单（改 client，零逻辑）

> 这是**唯一需要对 opc-da-client 做的改动**，性质是「开放已有内部资产」，不改任何逻辑/行为/API 语义。可作为 client 的独立改进提交，不绑定 server 项目。

`opc-da-client/src/lib.rs`：
- `mod bindings;` → `pub mod bindings;`
- `mod opc_da;` → `pub mod opc_da;`（连带暴露 `com_utils` / `typedefs` / `traits` / `client`）
  - 更精细的替代：保持 `mod opc_da` 私有，改用 `pub use opc_da::{com_utils, typedefs};`（按需暴露，最小开口）。**推荐精细版**，避免一次性暴露整个 opc_da 模块树。
- `bindings` 是 `#[allow(warnings)]` 的冻结 winmd 产物；公开为 public API 需在 lib.rs 文档里加一句约束：「`bindings` 为 windows-bindgen 机器产物，视为稳定基础设施，不接受语义性手改」。

> 其余 client 代码（backend / com_worker / provider / fusion_reader / subscription）**零改动**。

---

## 5. COM 地基（阶段 0）

### 5.1 IClassFactory
- `#[implement(windows::Win32::System::Com::IClassFactory)]`（来自 windows-rs，非 OPC bindings）。
- `CreateInstance(outer, iid, out)`：不接受 aggregation（`outer` 非空返回 `CLASS_E_NOAGGREGATION`）；按被请求的 CLSID 创建 Server 对象，`QueryInterface(iid)` 填 `out`。
- `LockServer(lock)`：调 `CoAddRefServerProcess` / `CoReleaseServerProcess` 维持进程存活。

### 5.2 类对象生命周期（EXE 启动）
```
main():
  CoIncrementMTAUsage()                       // 或 CoInitializeEx(MTA)
  CoInitializeSecurity(...)                   // DCOM 前置（阶段 3 必须；阶段 0-2 本机可宽松）
  for clsid in [CLSID_OPCDA_SERVER]:
      factory = Factory.into()
      cookie = CoRegisterClassObject(clsid, &factory,
                  CLSCTX_LOCAL_SERVER,
                  REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED)
  CoResumeClassObjects()                      // 一次性通知 SCM
  CoAddRefServerProcess()                     // 人工 +1（代表"注册自身"）

  // —— 主循环：等到所有实例/锁释放 ——
  while CoReleaseServerProcess() != 0 { 阻塞等待信号 }
  // 或保持进程存活的事件等待 + IOPCShutdown 触发

  CoRevokeClassObject(cookie) for each
  // CoUninitialize()
```
- `REGCLS_MULTIPLEUSE`（所有 client 共用一个 server 进程，OPC 标配）+ `REGCLS_SUSPENDED`（多类原子注册，防激活竞态）。
- 退出时机：per-process 引用计数归零（`CoReleaseServerProcess()==0`）。每个 `CreateInstance` 成功后 COM 内部 `CoAddRefServerProcess`，client 释放对象时递减。

### 5.3 EXE 主循环
- MTA（`CoIncrementMTAUsage`）—— OPC DA server 主流 free-threaded，`#[implement]` 默认 agile（自带 `IMarshal`/`IAgileObject`），**保持默认**。
- 主线程：事件/信号量阻塞到「该退出」（引用计数归零 或 `/Stop` 或 Service 停止）。MTA 下不强制消息泵。
- 命令行：`/RegServer` / `/UnregServer`（写/删注册表，注册后立即退出）；无参 = 启动服务循环。

> ⚠ windows-rs 版本：本项目 pin `windows = 0.61.3`（workspace）。`IClassFactory_Impl::CreateInstance` 首参在 0.61 vs master(0.100) 签名不同。**以本地 `cargo doc --package windows` 为准**，不照抄调研骨架。

---

## 6. Server 对象（阶段 1）

```
#[implement(
    IOPCServer,
    IOPCCommon,
    IConnectionPointContainer,        // windows-rs
)]
struct ServerObj {
    inner: Arc<ServerInner>,          // group 注册表 + DataSource + 配置
    shutdown_cp: ConnectionPoint<IOPCShutdown>,   // 暴露给 client advise
}
```
- **持有状态**（`ServerInner`，`Send+Sync`，锁守护）：
  - `groups: HashMap<hServerGroup, Arc<GroupObj>>`
  - `data_source: Arc<dyn DataSource>`
  - `vendor_info / major.minor.build / locale`
- **`IOPCServer_Impl`**：
  - `AddGroup(name, active, update_rate, ...) -> hServerGroup`：创建 Group 对象，注册，返回句柄。
  - `RemoveGroup(hServerGroup, bForce)`：drop group（连带 unadvise 所有 sink）。
  - `GetGroupByName` / `GetGroupByServerHandle`。
  - `GetStatus(out OPCSERVERSTATUS*)`：用 `typedefs::ServerStatus::to_native()` 构造（vendor/version/state/group_count/timestamps）。
  - `CreateGroupEnumerator`：`IEnumUnknown`（group 枚举，server 实现）。
  - `GetErrorString`：委托 `IOPCCommon`。
- **`IOPCCommon_Impl`**：`SetLocaleID` / `QueryAvailableLocales` / `GetErrorString`（用 helpers HRESULT 表）/ `SetClientName`。
- **`IConnectionPointContainer_Impl`**：`EnumConnectionPoints` / `FindConnectionPoint(riid)`——只认 `IID_IOPCShutdown`，返回 `shutdown_cp`。

---

## 7. Group 对象（阶段 1）

```
#[implement(
    IOPCItemMgt,
    IOPCGroupStateMgt,
    IOPCSyncIO,
    IOPCAsyncIO2,
    IConnectionPointContainer,        // windows-rs
    // 阶段 2 追加: IOPCGroupStateMgt2, IOPCSyncIO2, IOPCAsyncIO3
)]
struct GroupObj {
    inner: Arc<Mutex<GroupInner>>,
    data_cp: ConnectionPoint<IOPCDataCallback>,
    publisher: Option<PublisherHandle>,   // 订阅推送任务
}
```
- **`GroupInner`**：
  - `items: HashMap<hServerItem, ItemEntry{ item_id, hClient, active, data_type, deadband }>`
  - `update_rate / active / name / time_bias / percent_deadband / locale`
  - `data_source: Arc<dyn DataSource>` 引用
- **`IOPCItemMgt_Impl`**：`AddItems(OPCITEMDEF[]) -> OPCITEMRESULT[]`（注册 item + 分配 server handle + 从 DataSource 查 data_type/access_rights）；`RemoveItems` / `ValidateItems` / `SetActiveState` / `SetClientHandles` / `GetItemIDs`。
- **`IOPCGroupStateMgt_Impl`**：`GetState` / `SetState(update_rate/active/deadband/locale)` / `SetName` / `CloneGroup`。
- **`IOPCSyncIO_Impl`**：`Read(OPC_DS, hServerItems[]) -> OPCITEMSTATE[]`（从 DataSource 读 → VARIANT+quality+timestamp）；`Write(hServerItems[], values[])`。
- **`IOPCAsyncIO2_Impl`**：`Read/Write/Refresh2` 立即返回 `CancelID`，结果走 client 的 `IOPCDataCallback::OnReadComplete/OnWriteComplete/OnDataChange`。
- **`IConnectionPointContainer_Impl`**：`FindConnectionPoint(DIID_IOPCDataCallback)` 返回 `data_cp`。

---

## 8. IConnectionPoint 通用实现

`IConnectionPointContainer` / `IConnectionPoint` 来自 **windows-rs `Win32::System::Com`**（不在 OPC bindings，需 server 自实现）。
- 通用 `ConnectionPoint<DIID>`：`Advise(pUnkSink) -> cookie` / `Unadvise(cookie)` / `EnumConnections`，内部 `HashMap<cookie, ComPtr<sink>>`。
- 两个实例：
  - Server 上：`ConnectionPoint<IOPCShutdown>`（DIID_IOPCShutdown）。
  - 每个 Group 上：`ConnectionPoint<IOPCDataCallback>`（DIID_IOPCDataCallback）。
- 线程安全：sink 指针跨线程调用，依赖 MTA + agile；server 调 sink 前需处理失效（`RPC_E_DISCONNECTED` → 移除该 advise）。

---

## 9. 数据源抽象（trait + SimDataSource）

```rust
/// server 的"虚拟工厂"——所有数据的来源。
pub trait DataSource: Send + Sync {
    /// 命名空间（browse 用）：分支/叶子树。
    fn namespace(&self) -> &NamespaceTree;
    /// 同步读一个 item（COM 调用线程直接读缓存）。
    fn read(&self, item_id: &str) -> (VARIANT, u16 /*quality*/, FILETIME);
    /// 写一个 item。
    fn write(&self, item_id: &str, value: &VARIANT) -> HRESULT;
    /// 该 item 的规范 data_type / access_rights（AddItems 用）。
    fn item_meta(&self, item_id: &str) -> Option<ItemMeta>;
}
```
- **`SimDataSource`**（默认实现，内置）：镜像 Matrikon.OPC.Simulation 的标签集（`Random.Int4` / `Random.Real8` / `Bucket Brigade.Int4` / `Square Wave.Real8` / 计数器等）；值产生器（sine / uniform random / 递增计数器）由后台 tokio task 周期更新缓存；`read` 返回当前缓存。
- 设计要点：`read` 同步（COM 方法在 worker 线程同步调）；缓存由独立后台 task 异步刷新——解耦「COM 同步语义」与「数据产生异步」。
- 这是未来「协议网关 DataSource」（Modbus/S7/UA 桥接）的扩展点，本方案只做 Sim。

---

## 10. 订阅推送引擎（阶段 1）

- 每个 active 且有 advise 的 group 起一个推送任务（tokio task 或专用线程）：
  - 周期 = group `update_rate`（ms）。
  - 扫描该 group 的 items，`DataSource::read` 取最新值。
  - **deadband 过滤**：变化幅度 < `percent_deadband` 的不推送（quality 变化总是推）。
  - **打包并行数组**：`hClientItems[] / vValues[] / wQualities[] / ftTimeStamps[] / hErrors[]`，全部 `CoTaskMemAlloc` 分配。
    - ⚠ 所有权陷阱：这些数组的所有权**转移给 client**（client 的 proxy 释放）。com_utils 的 `RemoteArray` RAII 会自动 free——**这里要绕开 RAII**：用「分配 + `mem::forget`」或裸 `CoTaskMemAlloc` 不经 RAII 的 helper（server 侧新增 `OwningRemoteArray`，分配后交出所有权不 free）。
  - **调 sink**：`unsafe { sink.OnDataChange(...) }`（bindings 提供 client 侧调用 fn），遍历 `data_cp` 的 advise 表。失败（`RPC_E_DISCONNECTED`）→ 标记移除。
- 健壮性：`set_keep_alive`（阶段 2 的 `IOPCGroupStateMgt2`）——无数据变化时周期发空心跳，防 client 误判连接死活（呼应 client 侧 P0-1 自愈）。

---

## 11. 注册工具（阶段 0/1）

`registry.rs` + `/RegServer` `/UnregServer` 命令行：
- `HKCR\CLSID\{CLSID}\LocalServer32 = "<exe 路径>"`
- `...\ProgID = "Vendor.OPC.DA.Server.1"` / `...\VersionIndependentProgID = "Vendor.OPC.DA.Server"`
- `...\Implemented Categories\{CATID_OPCDAServer20}`（+ `30` 阶段 2）
- `HKCR\AppID\{AppID}` + `...\AppID = {AppID}`（DCOM 聚合，阶段 3 远程必需）
- **位宽**（已知坑）：显式选 32/64 位注册表视图（`KEY_WOW64_32KEY` / `KEY_WOW64_64KEY`），否则 client 跨位宽 `list_servers` 枚举不到。
- 可选：`ICatRegister::RegisterClassImplCategories`（比手写键稳）。

---

## 12. DCOM 远程激活（阶段 3，风险最高）

- **proxy/stub**（语言无关硬要求）：OPC DA 自定义接口跨机编组**必须有 OPC proxy/stub DLL**（OPC Core Components 的 `OPCproxy.dll`）——这是**外部依赖 + 配置**，非本 crate 编码。本机/同机激活不需要；跨机必需。
- **EXE 形态**：生产远程激活要求 Windows Service 包装或 `RunAs` 账户（console EXE 只能本地激活）。
  - 用 `windows-service` crate 或自写 Service 入口；`RunAs="Interactive User"` 仅本地交互有效，远程需特定账户。
- **安全**：注册类对象**之前**调 `CoInitializeSecurity`（认证级别 `Connect` 默认；`LaunchPermission`/`AccessPermission` 放行远程 client）；`dcomcnfg` 或代码配 `SD`。
- **防火墙**：TCP 135 + DCOM 动态端口范围。

---

## 13. 测试闭环

| 层级 | 方案 | 价值 |
|---|---|---|
| **自闭环（主力）** | opc-da-client(`OpcDaClient`) ↔ opc-da-server，同进程 `CoRegisterClassObject` 注册或本机 `LocalServer` | 摆脱 Matrikon；白盒构造边界（空命名空间/超深/订阅竞态/32↔64 位）；可进 Windows CI |
| **互操作护栏** | Matrikon OPC Explorer / KEPServer Explorer / Takebishi 连自建 server | 第三方 client 能 browse+订阅 = IID/marshalling/connection-point 全对的金标准 |
| **单元** | `DataSource` / deadband 过滤 / 并行数组打包 纯逻辑测试（不依赖 COM 激活） | 快速回归 |
| **现有 e2e** | opc-da-client 现有 19 本地 + 4 远程 e2e（连 Matrikon）保留 | 防「self-self 都对但共享同一 COM 误解」的盲区 |

- 自闭环 e2e 目标进 `verify.ps1`（Windows job），让 CI 不再依赖 Matrikon 安装。

---

## 14. 分阶段路线 + 工作量（单人全职等价，±50%）

| 阶段 | 内容 | 估时 | 退出标准 |
|---|---|---|---|
| **0. COM 地基** | client 可见性暴露 + `IClassFactory` + `CoRegisterClassObject` 生命周期 + EXE 主循环 + 注册工具 + 最小 `IOPCServer`/`IOPCCommon` | 1–2 周 | opc-da-client `OpcDaClient::default()` 经 `CoCreateInstance` 拿到自建 server 的 `IOPCServer`，`get_server_status` 通 |
| **1. 最小 DA 2.0（本地）** | Server 完整 + Group(IOPCItemMgt/GroupStateMgt/SyncIO/AsyncIO2) + 两份 ConnectionPoint + SimDataSource + 订阅推送引擎 | 3–5 周 | opc-cli / opc-da-desktop 能对自建 server browse?(阶段2前用 flat) / **read / write / subscribe**，自闭环 e2e 通过 |
| **2. Browse + DA3** | `IOPCBrowseServerAddressSpace` + `IOPCItemProperties` + `IOPCGroupStateMgt2`(keepalive) + `IOPCSyncIO2`(MaxAge/VQT) + `IOPCAsyncIO3` | 2–3 周 | opc-cli/desktop 完整 browse + 订阅体验；DA3 CATID 注册 |
| **3. DCOM 远程** | proxy/stub 配置 + Windows Service/RunAs + `CoInitializeSecurity` + dcomcnfg | 2–4 周 | Matrikon Explorer / KEPServer Explorer **跨机** browse+read+subscribe |
| **4. 硬化 + CI** | unsafe/内存 review、并发同步、`/UnregServer`、自闭环 e2e 入 verify | 1–2 周 | `verify.ps1` 含 server 自闭环；clippy/doc test 过 |

**合计 2–4 人月**。最大不确定性：阶段 3（DCOM/proxy-stub/Service）与 clean-room 调试周期。

---

## 15. 风险与前置 spike

| 风险 | 等级 | 缓解 |
|---|---|---|
| **`#[implement]` 多接口共存**（Server 要 `IOPCServer`+`IOPCCommon`+`IConnectionPointContainer` 三接口；subscription.rs 只验证过单接口 sink） | 高 | **阶段 0 早期 spike**：先实现一个空壳多接口 Server 对象，`CoCreateInstance` + 三个 `cast()` 都成功，再铺开 |
| **反向回调跨线程/跨机编组**（sink 指针 marshal） | 高 | MTA + agile 默认；spike 跨线程调 `OnDataChange`；远程靠 proxy/stub |
| **proxy/stub 外部依赖**（OPC Core Components） | 中 | 阶段 3 文档说明；本机/同机阶段不卡 |
| **无 Rust 先例**（clean-room） | 中 | 以 OPC spec + MIDL 产物为准；第三方 client 护栏 |
| **位宽**（已知坑，同 client） | 中 | 注册工具显式选 view；自闭环覆盖 32↔64 |
| **EXE 生命周期 / Service 复杂度** | 中 | 阶段 0-2 console EXE 先跑通；Service 留阶段 3 |
| **windows-rs 0.61 签名差异**（vs 调研的 master 0.100） | 低 | 以本地 `cargo doc` 为准 |

---

## 16. 关键设计决策（待实现时确认 / 可讨论）

1. **client 暴露粒度**：`pub mod opc_da`（整模块树）vs `pub use opc_da::{com_utils, typedefs}`（最小开口）。**推荐最小开口**。
2. **`DataSource::read` 同步 vs async**：推荐**同步读缓存**（COM 调用线程）+ 后台 task 异步刷新缓存——解耦 COM 同步语义与数据产生。
3. **推送引擎线程模型**：tokio task（与 opc-da-client 一致）vs 专用 std thread。推荐 tokio task。
4. **EXE 形态分阶段**：阶段 0-2 console EXE（本地激活）；阶段 3 Windows Service（远程）。
5. **多接口 `#[implement]` spike 前置**：阶段 0 第一周做，验证后再铺开（最高风险点前置）。

---

## 17. 与 client 的协同 / BUG 零容忍

- server 实现过程中若发现 opc-da-client 真 bug（如 CATID 此类——已核实不是），**必须修 client**，不绕过。
- server 依赖的 client 公开资产，若发现接口设计缺陷导致 server 无法正确实现（非 bug，是设计），**不改 client**，而是在 server 侧用自己的实现绕开（符合硬约束）。
- server 的自闭环 e2e 反过来也是 client 的回归测试——发现 client 侧回归时按 bug 处理。

---

## 18. 实现笔记（阶段 0 spike 已验证，2026-08-02）

- ✅ **多接口 `#[implement]` 可行**——spike `multi_interface_qi_succeeds` 通过：`#[implement(IOPCServer, IOPCCommon)]` 在 windows 0.61 + 冻结 bindings 下，QI 到 IOPCServer / IOPCCommon / IUnknown 三接口全部成功（vtable offset 正确）。**最高风险点排除。**
- ⚠ **多接口 impl 目标必须是 `_Impl` 类型**：`impl IOPCServer_Impl for ServerObj_Impl`（宏生成的 `ServerObj_Impl`），**不是原始 `ServerObj`**。单接口可 impl 原始类型（如 `subscription.rs` 的 `DataCallbackSink`），但**多接口必须 impl `_Impl` 类型**——否则 `vtable::new::<Identity>` 的 `Identity: *_Impl` bound 不满足。
- ⚠ **windows 0.61 类型路径**：`BOOL` / `HRESULT` / `GUID` 在 `windows::core::*`（**不是** `Win32::Foundation``）；只有 `E_NOTIMPL` 等 HRESULT 常量在 `windows::Win32::Foundation`。bindings 用 `windows_core::BOOL/HRESULT` 印证。
- ✅ **对象构造**：原始 `ServerObj` 可 `.into() -> IUnknown`（`#[implement]` 宏生成转换），用于测试与 `CreateInstance` 返回。
- ✅ **client 可见性暴露验证零破坏**：`pub mod bindings` + `pub use opc_da::{com_utils, typedefs}` 后，opc-da-client clippy（`-D warnings`）+ 15 doctest 全过。
- ✅ **IClassFactory + COM 自激活验证**（测试 `self_activate_via_coregister` 通过）：同进程 `CoRegisterClassObject(REGCLS_MULTIPLEUSE|SUSPENDED)` + `CoResumeClassObjects` + `CoCreateInstance` 命中本地工厂，`ServerObj` 被实例化，QI 到 IOPCServer / IOPCCommon 成功。**COM 激活链打通**——阶段 0 核心里程碑。
- 📝 **windows 0.61 COM 函数签名**（实测）：
  - `CoRegisterClassObject` 4 参返回 `Result<u32>`（cookie 在返回值，**非** out 参数）。
  - `IClassFactory_Impl::CreateInstance` 第二参 `Ref<IUnknown>`、第三/四参 `*const GUID` / `*mut *mut c_void`；`LockServer(BOOL)`。
  - `Interface::query(&self, iid, ppv) -> HRESULT`（trait 方法，0.61 返回 HRESULT；`.ok()` 转 `Result<()>`）。
