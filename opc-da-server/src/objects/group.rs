//! Group 对象。
//!
//! [`GroupObj`] 实现 `IOPCItemMgt` + `IOPCSyncIO` + `IOPCGroupStateMgt` + `IConnectionPointContainer`
//! （`FindConnectionPoint` 已实装；`EnumConnectionPoints` 暂 `E_NOTIMPL`）+ `IOPCAsyncIO2`（Refresh2
//! 已实装；Read/Write/Cancel2/SetEnable/GetEnable 暂 `E_NOTIMPL`）。订阅推送由全局 `scheduler`
//! 周期调 `publisher::push_data_change` → `OnDataChange`（`GroupObj::new` 注册 job 到调度器）。
//! `IOPCItemMgt`：`AddItems` / `ValidateItems` / `RemoveItems` / `SetActiveState` /
//! `SetClientHandles`——item 注册表（[`GroupInner`]）+ DataSource 元数据 + COM 内存。
//! `IOPCSyncIO`：`Read`（DataSource::read → OPCITEMSTATE[]{hClient,ft,quality,vDataValue}）
//! / `Write`（VARIANT → DataSource::write）；未知 handle 返回 `E_INVALIDARG`。
//! `IOPCGroupStateMgt`：`GetState`（读 group state + name wide）/ `SetState`（null=不改）
//! / `SetName`；`CloneGroup` 暂 `E_NOTIMPL`。
//!
//! `SetDatatypes` / `CreateEnumerator` 暂 `E_NOTIMPL`（后续阶段）。

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, FILETIME, S_OK,
};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, CoTaskMemFree, IConnectionPoint, IConnectionPointContainer,
    IConnectionPointContainer_Impl, IEnumConnectionPoints,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT};
use windows::core::{
    BOOL, Error, GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Result, implement,
};

use opc_da_client::bindings::da::{
    IOPCAsyncIO2, IOPCAsyncIO2_Impl, IOPCDataCallback, IOPCGroupStateMgt, IOPCGroupStateMgt_Impl,
    IOPCItemMgt, IOPCItemMgt_Impl, IOPCSyncIO, IOPCSyncIO_Impl, tagOPCDATASOURCE, tagOPCITEMDEF,
    tagOPCITEMRESULT, tagOPCITEMSTATE,
};

use crate::data_source::DataSource;
use crate::objects::ConnectionPoint;

/// OPC item 访问权限掩码（OPC DA 规范 §2.3）。
const OPC_READABLE: u32 = 0x1;
const OPC_WRITEABLE: u32 = 0x2;

/// OPC DA 错误码：item ID 不存在（`OPC_BASE=0xC0040000` + 7）。bindings 未导出，按规范值定义。
#[allow(clippy::cast_possible_wrap)] // HRESULT 失败码用负 i32 表示（COM 约定）
const OPC_E_INVALIDITEMID: HRESULT = HRESULT(0xC004_0007u32 as i32);

/// 取锁；mutex poison 时返回 guard（不 panic）。poison 表示持锁线程曾 panic；本模块锁内
/// 不执行会 panic 的操作，故继续而非传播 panic（遵循 CLAUDE.md "禁止 panic"）。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 读 `PWSTR`（0 结尾 UTF-16）→ `String`。null 指针返回空串。
pub fn pwstr_to_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let ptr = p.as_ptr();
    // SAFETY: 调用方保证 p 指向 0 结尾 UTF-16 串；循环到 null 终止，len 不越界。
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(ptr, len))
    }
}

/// 已注册 item 的服务端记录。
pub struct ItemEntry {
    /// item 全路径 id（`Arc<str>` clone 廉价——snapshot/推送路径避免 String clone，P3.1）。
    pub item_id: Arc<str>,
    pub h_client: u32,
    pub active: bool,
    #[allow(dead_code)] // 规范类型记录；当前 read 经 DataSource 按 item_id，未直接读此字段
    data_type: VARENUM,
    /// 上次推送状态（deadband 用，P1）。`None` = 未推过（首次必推）。
    pub last_pushed: Option<crate::data_source::PushState>,
}

/// Group 的可变状态（item 注册表 + handle 分配器 + group state）。`Mutex` 守护，跨 COM 调用线程。
pub struct GroupInner {
    pub items: HashMap<u32, ItemEntry>,
    next_handle: u32,
    // —— IOPCGroupStateMgt 状态（GetState/SetState/SetName 读写）——
    update_rate: u32,
    active: bool,
    name: String,
    time_bias: i32,
    pub percent_deadband: f32,
    locale: u32,
    pub h_client_group: u32,
    h_server_group: u32,
}

impl GroupInner {
    /// 分配下一个未用的 server handle（0 永不返回——0 = 无效）。
    fn alloc_handle(&mut self) -> u32 {
        loop {
            let h = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if h != 0 && !self.items.contains_key(&h) {
                return h;
            }
        }
    }

    /// `Refresh2` 用：快照 active items 的 `(h_client_group, [(h_client, item_id)])`。
    /// 与 `snapshot_for_publish` 区别：只含 `active` 的 item（OPC DA 规范：Refresh 只推 active）。
    pub fn snapshot_active_for_publish(&self) -> (u32, Vec<(u32, Arc<str>)>) {
        let frames = self
            .items
            .values()
            .filter(|e| e.active)
            .map(|e| (e.h_client, Arc::clone(&e.item_id)))
            .collect();
        (self.h_client_group, frames)
    }
}

/// OPC DA Group COM 对象。
///
/// 持有 item 注册表（`inner`）+ 数据源引用（`data_source`，`AddItems` 查 meta / 后续
/// `Read`/`Write` 取值）+ 订阅连接点（`data_cp`，client `Advise` `IOPCDataCallback`）。
#[implement(
    IOPCItemMgt,
    IOPCGroupStateMgt,
    IOPCSyncIO,
    IOPCAsyncIO2,
    IConnectionPointContainer
)]
pub struct GroupObj {
    inner: Arc<Mutex<GroupInner>>,
    data_source: Arc<dyn DataSource>,
    /// 订阅 sink 连接点（`IConnectionPoint` 接口，指向 `ConnectionPoint<IOPCDataCallback>`
    /// COM 对象，refcount 由 COM 自管）。`FindConnectionPoint(IOPCDataCallback)` 返回它的
    /// clone（AddRef）。
    pub(crate) data_cp: IConnectionPoint,
    /// 订阅表共享句柄（`data_cp` 内部 sinks 的 `Arc`，`cookie → GIT cookie`）。`Refresh2` /
    /// scheduler 推送经 `publisher::typed_sinks` → GIT 取 proxy 调 `OnDataChange`——免 STA
    /// client sink 跨线程 `RPC_E_WRONG_THREAD`（0x8001010E）。
    data_sinks: Arc<Mutex<HashMap<u32, u32>>>,
    /// `IOPCAsyncIO2` 事务 CancelID 分配器（从 1 起，0 = 无效）。
    next_cancel_id: AtomicU32,
}

impl GroupObj {
    /// 新建 Group（空 item 注册表 + 初始 group state，绑定 DataSource）。
    ///
    /// group state 由 `IOPCServer::AddGroup` 的参数初始化（name/active/update_rate/
    /// time_bias/percent_deadband/locale/h_client/h_server）；后续 `IOPCGroupStateMgt`
    /// 的 `GetState`/`SetState`/`SetName` 读写。
    #[allow(clippy::too_many_arguments)] // COM AddGroup 参数集，作 group state 初值
    pub(crate) fn new(
        data_source: Arc<dyn DataSource>,
        name: String,
        active: bool,
        update_rate: u32,
        time_bias: i32,
        percent_deadband: f32,
        locale: u32,
        h_client_group: u32,
        h_server_group: u32,
    ) -> Self {
        let inner = Arc::new(Mutex::new(GroupInner {
            items: HashMap::new(),
            next_handle: 1,
            update_rate,
            active,
            name,
            time_bias,
            percent_deadband,
            locale,
            h_client_group,
            h_server_group,
        }));
        // data_cp 未 attach container（见 ConnectionPoint::GetConnectionPointContainer 注释）。
        // sinks_arc 共享给 scheduler（推送免 QI 取 sink 快照，见 publisher::typed_sinks）。
        let cp = ConnectionPoint::<IOPCDataCallback>::new();
        let data_sinks = cp.sinks_arc();
        let data_cp: IConnectionPoint = cp.into();
        // 注册全局推送调度器（替代旧 per-group spawn；Scheduler 未 init 时 global() 返
        // None 跳过——兼容单测。h_server_group 作 GroupKey，update_rate 作周期）。
        if let Some(sched) = crate::objects::scheduler::global() {
            sched.register(
                h_server_group,
                Arc::clone(&inner),
                data_source.clone(),
                data_sinks.clone(),
                update_rate,
            );
        }
        Self {
            inner,
            data_source,
            data_cp,
            data_sinks,
            next_cancel_id: AtomicU32::new(1),
        }
    }
}

/// GroupObj 释放时从调度器注销推送任务（幂等；Scheduler 未 init 时跳过）。
impl Drop for GroupObj {
    fn drop(&mut self) {
        if let Some(sched) = crate::objects::scheduler::global() {
            let key = locked(&self.inner).h_server_group;
            sched.unregister(key);
        }
    }
}

/// 通用"未实装"错误：尚未实装的方法返回 `E_NOTIMPL`。
fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

/// 读 frames 的全部 item → 4 并行数组（`Refresh2` 全推用，绕过 deadband）。
fn read_frames(
    frames: &[(u32, Arc<str>)],
    data_source: &dyn DataSource,
) -> (Vec<u32>, Vec<VARIANT>, Vec<u16>, Vec<FILETIME>) {
    let mut hclients = Vec::with_capacity(frames.len());
    let mut values = Vec::with_capacity(frames.len());
    let mut qualities = Vec::with_capacity(frames.len());
    let mut timestamps = Vec::with_capacity(frames.len());
    for (hc, id) in frames {
        let (v, q, ts) = data_source.read(id);
        hclients.push(*hc);
        values.push(v);
        qualities.push(q);
        timestamps.push(ts);
    }
    (hclients, values, qualities, timestamps)
}

impl IOPCItemMgt_Impl for GroupObj_Impl {
    fn AddItems(
        &self,
        dwcount: u32,
        pitemarray: *const tagOPCITEMDEF,
        ppaddresults: *mut *mut tagOPCITEMRESULT,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        let n = prealloc_item_results(dwcount, pitemarray, ppaddresults, pperrors)?;
        tracing::debug!(method = "AddItems", count = n);
        let mut inner = locked(&self.inner);
        for i in 0..n {
            // SAFETY: pitemarray 含 dwcount 个 OPCITEMDEF（调用方保证）；i < n = dwcount。
            let def = unsafe { &*pitemarray.add(i) };
            let item_id = pwstr_to_string(def.szItemID);
            match self.data_source.item_meta(&item_id) {
                Some(meta) => {
                    tracing::debug!(method = "AddItems", item = %item_id, result = "ok");
                    let h_server = inner.alloc_handle();
                    inner.items.insert(
                        h_server,
                        ItemEntry {
                            item_id: Arc::from(item_id),
                            h_client: def.hClient,
                            active: def.bActive.as_bool(),
                            data_type: meta.data_type,
                            last_pushed: None,
                        },
                    );
                    // SAFETY: ppaddresults 由 prealloc 的 CoTaskMemAlloc 分配（未初始化裸内存）；i < n。
                    // ptr::write 不读/不 drop 旧值——对未初始化内存安全（OPCITEMRESULT 当前无 Drop，
                    // 但统一 ptr::write 防未来加字段引入 Drop 类 UB）。
                    unsafe {
                        std::ptr::write(
                            (*ppaddresults).add(i),
                            tagOPCITEMRESULT {
                                hServer: h_server,
                                vtCanonicalDataType: meta.data_type.0,
                                wReserved: 0,
                                dwAccessRights: if meta.writable {
                                    OPC_READABLE | OPC_WRITEABLE
                                } else {
                                    OPC_READABLE
                                },
                                dwBlobSize: 0,
                                pBlob: core::ptr::null_mut(),
                            },
                        );
                        *(*pperrors).add(i) = S_OK;
                    }
                }
                None => {
                    tracing::debug!(method = "AddItems", item = %item_id, result = "invalid");
                    // SAFETY: 同上；ptr::write 写零结果到未初始化槽 + INVALIDITEMID。
                    unsafe {
                        std::ptr::write(
                            (*ppaddresults).add(i),
                            tagOPCITEMRESULT {
                                hServer: 0,
                                vtCanonicalDataType: 0,
                                wReserved: 0,
                                dwAccessRights: 0,
                                dwBlobSize: 0,
                                pBlob: core::ptr::null_mut(),
                            },
                        );
                        *(*pperrors).add(i) = OPC_E_INVALIDITEMID;
                    }
                }
            }
        }
        drop(inner);
        Ok(())
    }

    fn ValidateItems(
        &self,
        dwcount: u32,
        pitemarray: *const tagOPCITEMDEF,
        _bblobupdate: BOOL,
        ppvalidationresults: *mut *mut tagOPCITEMRESULT,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        // 与 AddItems 同结构，但不注册（不分配 hServer / 不写 inner）。
        let n = prealloc_item_results(dwcount, pitemarray, ppvalidationresults, pperrors)?;
        for i in 0..n {
            // SAFETY: pitemarray 含 dwcount 个 OPCITEMDEF；i < n。
            let def = unsafe { &*pitemarray.add(i) };
            let item_id = pwstr_to_string(def.szItemID);
            match self.data_source.item_meta(&item_id) {
                Some(meta) => {
                    // SAFETY: ppvalidationresults 由 prealloc 的 CoTaskMemAlloc 分配（未初始化裸内存）；
                    // i < n。ptr::write 不读/不 drop 旧值——对未初始化内存安全（OPCITEMRESULT 当前无
                    // Drop，但统一 ptr::write 防未来加字段引入 Drop 类 UB）。
                    unsafe {
                        std::ptr::write(
                            (*ppvalidationresults).add(i),
                            tagOPCITEMRESULT {
                                hServer: 0,
                                vtCanonicalDataType: meta.data_type.0,
                                wReserved: 0,
                                dwAccessRights: if meta.writable {
                                    OPC_READABLE | OPC_WRITEABLE
                                } else {
                                    OPC_READABLE
                                },
                                dwBlobSize: 0,
                                pBlob: core::ptr::null_mut(),
                            },
                        );
                        *(*pperrors).add(i) = S_OK;
                    }
                }
                None => {
                    // SAFETY: 同上；ptr::write 写零结果到未初始化槽 + INVALIDITEMID。
                    unsafe {
                        std::ptr::write(
                            (*ppvalidationresults).add(i),
                            tagOPCITEMRESULT {
                                hServer: 0,
                                vtCanonicalDataType: 0,
                                wReserved: 0,
                                dwAccessRights: 0,
                                dwBlobSize: 0,
                                pBlob: core::ptr::null_mut(),
                            },
                        );
                        *(*pperrors).add(i) = OPC_E_INVALIDITEMID;
                    }
                }
            }
        }
        Ok(())
    }

    fn RemoveItems(
        &self,
        dwcount: u32,
        phserver: *const u32,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        let n = prealloc_errors(dwcount, phserver, pperrors)?;
        let mut inner = locked(&self.inner);
        for i in 0..n {
            // SAFETY: phserver 含 dwcount 个 u32；i < n。
            let h = unsafe { *phserver.add(i) };
            let hr = if inner.items.remove(&h).is_some() {
                S_OK
            } else {
                E_INVALIDARG
            };
            // SAFETY: pperrors 含 n 个槽；i < n。
            unsafe {
                *(*pperrors).add(i) = hr;
            }
        }
        drop(inner);
        Ok(())
    }

    fn SetActiveState(
        &self,
        dwcount: u32,
        phserver: *const u32,
        bactive: BOOL,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        let active = bactive.as_bool();
        let n = prealloc_errors(dwcount, phserver, pperrors)?;
        let mut inner = locked(&self.inner);
        for i in 0..n {
            // SAFETY: phserver 含 dwcount 个 u32；i < n。
            let h = unsafe { *phserver.add(i) };
            let hr = match inner.items.get_mut(&h) {
                Some(entry) => {
                    entry.active = active;
                    S_OK
                }
                None => E_INVALIDARG,
            };
            // SAFETY: pperrors 含 n 个槽；i < n。
            unsafe {
                *(*pperrors).add(i) = hr;
            }
        }
        drop(inner);
        Ok(())
    }

    fn SetClientHandles(
        &self,
        dwcount: u32,
        phserver: *const u32,
        phclient: *const u32,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        let n = prealloc_errors(dwcount, phserver, pperrors)?;
        let mut inner = locked(&self.inner);
        for i in 0..n {
            // SAFETY: phserver / phclient 各含 dwcount 个 u32；i < n。
            let (h, c) = unsafe { (*phserver.add(i), *phclient.add(i)) };
            let hr = match inner.items.get_mut(&h) {
                Some(entry) => {
                    entry.h_client = c;
                    S_OK
                }
                None => E_INVALIDARG,
            };
            // SAFETY: pperrors 含 n 个槽；i < n。
            unsafe {
                *(*pperrors).add(i) = hr;
            }
        }
        drop(inner);
        Ok(())
    }

    // TODO(后续阶段): SetDatatypes（强制 item 请求类型，覆盖 canonical）。
    fn SetDatatypes(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _prequesteddatatypes: *const u16,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    // TODO(后续阶段): CreateEnumerator（返 group items 枚举器）。
    fn CreateEnumerator(&self, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }
}

/// `AddItems` / `ValidateItems` 公共前置：校验指针 + 分配 `OPCITEMRESULT[]` 与 `HRESULT[]`
///（`CoTaskMemAlloc`，所有权交 client）。返回元素数 `n`。
fn prealloc_item_results(
    dwcount: u32,
    pitemarray: *const tagOPCITEMDEF,
    poutresults: *mut *mut tagOPCITEMRESULT,
    pperrors: *mut *mut HRESULT,
) -> Result<usize> {
    // dwcount==0：空操作成功——写 out=null 再返回 S_OK。部分 client（Prosys 等）会发
    // 0-item 请求做连接握手，且失败时仍读 out；若此处不写 out 返回 E_POINTER，client 读
    // 到未初始化垃圾指针 → 进程内 access violation。
    if dwcount == 0 {
        if !poutresults.is_null() {
            // SAFETY: 调用方提供的 out 指针；写 null 表示无结果数组（空操作）。
            unsafe { *poutresults = core::ptr::null_mut() };
        }
        if !pperrors.is_null() {
            // SAFETY: 同上。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Ok(0);
    }
    if pitemarray.is_null() || poutresults.is_null() || pperrors.is_null() {
        // E_POINTER 路径也把 out 置 null：client 若不查 HRESULT 直接读 out，至少拿到 null
        // 而非栈垃圾（避免访问非法地址）。
        if !poutresults.is_null() {
            // SAFETY: 同上。
            unsafe { *poutresults = core::ptr::null_mut() };
        }
        if !pperrors.is_null() {
            // SAFETY: 同上。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Err(Error::from(E_POINTER));
    }
    let n = dwcount as usize;
    // SAFETY: CoTaskMemAlloc 分配 size_of* n 字节；失败返回 null。
    let results_ptr =
        unsafe { CoTaskMemAlloc(size_of::<tagOPCITEMRESULT>() * n) }.cast::<tagOPCITEMRESULT>();
    let errors_ptr = unsafe { CoTaskMemAlloc(size_of::<HRESULT>() * n) }.cast::<HRESULT>();
    if results_ptr.is_null() || errors_ptr.is_null() {
        // SAFETY: 已分配的那个释放（可能 null，CoTaskMemFree 容忍 null）。
        unsafe {
            CoTaskMemFree(Some(results_ptr.cast()));
            CoTaskMemFree(Some(errors_ptr.cast()));
        }
        return Err(Error::from(E_OUTOFMEMORY));
    }
    // SAFETY: 调用方提供的 out 指针，写入分配的数组基址。
    unsafe {
        *poutresults = results_ptr;
        *pperrors = errors_ptr;
    }
    Ok(n)
}

/// `RemoveItems` / `SetActiveState` / `SetClientHandles` 公共前置：校验 + 分配 `HRESULT[]`。
fn prealloc_errors(
    dwcount: u32,
    phserver: *const u32,
    pperrors: *mut *mut HRESULT,
) -> Result<usize> {
    // dwcount==0：空操作成功（同 `prealloc_item_results`——写 null + S_OK，防 client 读垃圾 out）。
    if dwcount == 0 {
        if !pperrors.is_null() {
            // SAFETY: 调用方提供的 out 指针；写 null 表示无错误数组（空操作）。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Ok(0);
    }
    if phserver.is_null() || pperrors.is_null() {
        if !pperrors.is_null() {
            // SAFETY: 同上；失败路径也置 null，防 client 读未初始化 out。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Err(Error::from(E_POINTER));
    }
    let n = dwcount as usize;
    // SAFETY: CoTaskMemAlloc 分配 size_of*n 字节。
    let errors_ptr = unsafe { CoTaskMemAlloc(size_of::<HRESULT>() * n) }.cast::<HRESULT>();
    if errors_ptr.is_null() {
        return Err(Error::from(E_OUTOFMEMORY));
    }
    // SAFETY: 调用方 out 指针。
    unsafe {
        *pperrors = errors_ptr;
    }
    Ok(n)
}

/// `IOPCSyncIO::Read` 前置：校验指针 + 分配 `OPCITEMSTATE[]` 与 `HRESULT[]`
///（`CoTaskMemAlloc`，所有权交 client）。返回元素数 `n`。
fn prealloc_item_states(
    dwcount: u32,
    phserver: *const u32,
    ppitemvalues: *mut *mut tagOPCITEMSTATE,
    pperrors: *mut *mut HRESULT,
) -> Result<usize> {
    // dwcount==0：空操作成功（同 `prealloc_item_results`——写 null + S_OK，防 client 读垃圾 out）。
    if dwcount == 0 {
        if !ppitemvalues.is_null() {
            // SAFETY: 调用方提供的 out 指针；写 null 表示无值数组（空操作）。
            unsafe { *ppitemvalues = core::ptr::null_mut() };
        }
        if !pperrors.is_null() {
            // SAFETY: 同上。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Ok(0);
    }
    if phserver.is_null() || ppitemvalues.is_null() || pperrors.is_null() {
        // E_POINTER 路径也把 out 置 null（同 `prealloc_item_results`）。
        if !ppitemvalues.is_null() {
            // SAFETY: 同上。
            unsafe { *ppitemvalues = core::ptr::null_mut() };
        }
        if !pperrors.is_null() {
            // SAFETY: 同上。
            unsafe { *pperrors = core::ptr::null_mut() };
        }
        return Err(Error::from(E_POINTER));
    }
    let n = dwcount as usize;
    // SAFETY: CoTaskMemAlloc 分配 size_of*n 字节；失败返回 null。
    let values_ptr =
        unsafe { CoTaskMemAlloc(size_of::<tagOPCITEMSTATE>() * n) }.cast::<tagOPCITEMSTATE>();
    let errors_ptr = unsafe { CoTaskMemAlloc(size_of::<HRESULT>() * n) }.cast::<HRESULT>();
    if values_ptr.is_null() || errors_ptr.is_null() {
        // SAFETY: 已分配的释放（可能 null，CoTaskMemFree 容忍 null）。
        unsafe {
            CoTaskMemFree(Some(values_ptr.cast()));
            CoTaskMemFree(Some(errors_ptr.cast()));
        }
        return Err(Error::from(E_OUTOFMEMORY));
    }
    // SAFETY: 调用方提供的 out 指针，写入分配的数组基址。
    unsafe {
        *ppitemvalues = values_ptr;
        *pperrors = errors_ptr;
    }
    Ok(n)
}

impl IOPCGroupStateMgt_Impl for GroupObj_Impl {
    fn GetState(
        &self,
        pupdaterate: *mut u32,
        pactive: *mut BOOL,
        ppname: *mut PWSTR,
        ptimebias: *mut i32,
        ppercentdeadband: *mut f32,
        plcid: *mut u32,
        phclientgroup: *mut u32,
        phservergroup: *mut u32,
    ) -> Result<()> {
        if pupdaterate.is_null()
            || pactive.is_null()
            || ppname.is_null()
            || ptimebias.is_null()
            || ppercentdeadband.is_null()
            || plcid.is_null()
            || phclientgroup.is_null()
            || phservergroup.is_null()
        {
            return Err(Error::from(E_POINTER));
        }
        // 锁内读出标量 + 构造 name wide（encode 后 Vec 独立，锁可释放）。
        let name_wide = {
            let inner = locked(&self.inner);
            // SAFETY: 8 个 out 指针非空（上面校验）；从锁内 state 读标量写入。
            unsafe {
                *pupdaterate = inner.update_rate;
                *pactive = BOOL::from(inner.active);
                *ptimebias = inner.time_bias;
                *ppercentdeadband = inner.percent_deadband;
                *plcid = inner.locale;
                *phclientgroup = inner.h_client_group;
                *phservergroup = inner.h_server_group;
            }
            inner
                .name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        // name: CoTaskMemAlloc wide string（所有权交 client，client CoTaskMemFree）。
        // SAFETY: 分配 name_wide.len()*2 字节；失败返回 null。
        let name_ptr = unsafe { CoTaskMemAlloc(name_wide.len() * 2) }.cast::<u16>();
        if name_ptr.is_null() {
            return Err(Error::from(E_OUTOFMEMORY));
        }
        // SAFETY: name_ptr 刚分配足够空间；拷贝 name_wide（含 null 终止）。
        unsafe {
            std::ptr::copy_nonoverlapping(name_wide.as_ptr(), name_ptr, name_wide.len());
            *ppname = PWSTR(name_ptr);
        }
        Ok(())
    }

    fn SetState(
        &self,
        prequestedupdaterate: *const u32,
        previsedupdaterate: *mut u32,
        pactive: *const BOOL,
        ptimebias: *const i32,
        ppercentdeadband: *const f32,
        plcid: *const u32,
        phclientgroup: *const u32,
    ) -> Result<()> {
        // 各 in 指针 null = 不改该字段（OPC DA 语义）。as_ref() null → None。
        let revised = {
            let mut inner = locked(&self.inner);
            // SAFETY: 6 个 in 指针由调用方提供（COM 契约：指向有效 T 或 null）。
            // as_ref null → None（不改字段）；非 null 读值赋字段。
            unsafe {
                if let Some(&rate) = prequestedupdaterate.as_ref() {
                    inner.update_rate = rate;
                }
                if let Some(&a) = pactive.as_ref() {
                    inner.active = a.as_bool();
                }
                if let Some(&tb) = ptimebias.as_ref() {
                    inner.time_bias = tb;
                }
                if let Some(&pd) = ppercentdeadband.as_ref() {
                    inner.percent_deadband = pd;
                }
                if let Some(&lc) = plcid.as_ref() {
                    inner.locale = lc;
                }
                if let Some(&hc) = phclientgroup.as_ref() {
                    inner.h_client_group = hc;
                }
                inner.update_rate
            }
        };
        if !previsedupdaterate.is_null() {
            // SAFETY: previsedupdaterate 非空时为调用方 out 值。
            unsafe { *previsedupdaterate = revised };
        }
        Ok(())
    }

    fn SetName(&self, szname: &PCWSTR) -> Result<()> {
        let name = pwstr_to_string(PWSTR(szname.as_ptr().cast_mut()));
        locked(&self.inner).name = name;
        Ok(())
    }

    // TODO(后续阶段): CloneGroup（深拷贝 group state + items 到新 group）。
    fn CloneGroup(&self, _szname: &PCWSTR, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }
}

impl IOPCSyncIO_Impl for GroupObj_Impl {
    fn Read(
        &self,
        dwsource: tagOPCDATASOURCE,
        dwcount: u32,
        phserver: *const u32,
        ppitemvalues: *mut *mut tagOPCITEMSTATE,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        // dwsource 忽略：SimDataSource 无 cache/device 之分，统一 read-time 现算
        //（等价 device 读）。真 cache 语义待 publisher 引擎（§10）维护缓存后区分。
        let n = prealloc_item_states(dwcount, phserver, ppitemvalues, pperrors)?;
        tracing::debug!(method = "SyncIO::Read", source = dwsource.0, count = n);
        // 锁内取每个 handle 对应的 (hClient, item_id)；未知 handle 记 None。
        // 短持锁：仅查表 clone，DataSource::read 在锁外执行（避免长持锁期间做 IO）。
        let lookups: Vec<(u32, Option<Arc<str>>)> = {
            let inner = locked(&self.inner);
            (0..n)
                .map(|i| {
                    // SAFETY: phserver 含 dwcount 个 u32（调用方保证）；i < n。
                    let h = unsafe { *phserver.add(i) };
                    let entry = inner.items.get(&h);
                    (
                        entry.map_or(0, |e| e.h_client),
                        entry.map(|e| e.item_id.clone()),
                    )
                })
                .collect()
        };
        for (i, (h_client, maybe_id)) in lookups.into_iter().enumerate() {
            // SAFETY: ppitemvalues / pperrors 各含 n 个槽（prealloc 的 CoTaskMemAlloc 分配，未初始化
            // 裸内存）；i < n。ptr::write 不读/不 drop 旧值——对未初始化内存安全。
            // tagOPCITEMSTATE 含 VARIANT（有 Drop）：`*ptr=val` 会先隐式 drop 旧值（垃圾）→
            // VariantClear 读垃圾 vt → 间歇 SEH access violation → COM 报 0x80010105。
            unsafe {
                match maybe_id {
                    Some(item_id) => {
                        let (v, q, ts) = self.data_source.read(&item_id);
                        std::ptr::write(
                            (*ppitemvalues).add(i),
                            tagOPCITEMSTATE {
                                hClient: h_client,
                                ftTimeStamp: ts,
                                wQuality: q,
                                wReserved: 0,
                                vDataValue: v,
                            },
                        );
                        *(*pperrors).add(i) = S_OK;
                    }
                    None => {
                        // 未知 handle：空 ITEMSTATE（vDataValue=VT_EMPTY）+ E_INVALIDARG。
                        std::ptr::write((*ppitemvalues).add(i), tagOPCITEMSTATE::default());
                        *(*pperrors).add(i) = E_INVALIDARG;
                    }
                }
            }
        }
        Ok(())
    }

    fn Write(
        &self,
        dwcount: u32,
        phserver: *const u32,
        pitemvalues: *const VARIANT,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        let n = prealloc_errors(dwcount, phserver, pperrors)?;
        tracing::debug!(method = "SyncIO::Write", count = n);
        let lookups: Vec<Option<Arc<str>>> = {
            let inner = locked(&self.inner);
            (0..n)
                .map(|i| {
                    // SAFETY: phserver 含 dwcount 个 u32；i < n。
                    let h = unsafe { *phserver.add(i) };
                    inner.items.get(&h).map(|e| e.item_id.clone())
                })
                .collect()
        };
        for (i, maybe_id) in lookups.into_iter().enumerate() {
            let hr = match maybe_id {
                Some(item_id) => {
                    // SAFETY: pitemvalues 含 dwcount 个 VARIANT（调用方保证）；i < n。
                    let v = unsafe { &*pitemvalues.add(i) };
                    self.data_source.write(&item_id, v)
                }
                None => E_INVALIDARG,
            };
            // SAFETY: pperrors 含 n 个槽；i < n。
            unsafe {
                *(*pperrors).add(i) = hr;
            }
        }
        Ok(())
    }
}

impl IOPCAsyncIO2_Impl for GroupObj_Impl {
    // TODO(后续阶段): AsyncIO2::Read（异步读 + OnReadComplete 回调，需事务表）。
    fn Read(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _dwtransactionid: u32,
        _pdwcancelid: *mut u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    // TODO(后续阶段): AsyncIO2::Write（异步写 + OnWriteComplete 回调，需事务表）。
    fn Write(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _pitemvalues: *const VARIANT,
        _dwtransactionid: u32,
        _pdwcancelid: *mut u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn Refresh2(&self, _dwsource: tagOPCDATASOURCE, dwtransactionid: u32) -> Result<u32> {
        // dwsource（CACHE/DEVICE）忽略：SimDataSource 无 cache/device 之分。CancelID 仅满足
        // "唯一标识事务"契约——Refresh2 同步推送，返回时已完成；Cancel2 暂 nyi（无实际取消）。
        let cancel_id = self.next_cancel_id.fetch_add(1, Ordering::Relaxed);
        // 全推（绕过 deadband）：snapshot active → 读全部 → push。client 主动刷新要当前全量。
        let (h_group, frames) = locked(&self.inner).snapshot_active_for_publish();
        if frames.is_empty() {
            return Ok(cancel_id);
        }
        let sinks = crate::objects::publisher::typed_sinks(&self.data_sinks);
        if sinks.is_empty() {
            return Ok(cancel_id);
        }
        let (hclients, values, qualities, timestamps) = read_frames(&frames, &*self.data_source);
        crate::objects::publisher::push_data_change(
            &sinks,
            h_group,
            &hclients,
            &values,
            &qualities,
            &timestamps,
            dwtransactionid,
        );
        Ok(cancel_id)
    }

    // TODO(后续阶段): Cancel2（取消未完成 async 事务，需事务表）。
    fn Cancel2(&self, _dwcancelid: u32) -> Result<()> {
        nyi()
    }

    // TODO(后续阶段): SetEnable（group 回调总开关）。
    fn SetEnable(&self, _benable: BOOL) -> Result<()> {
        nyi()
    }

    // TODO(后续阶段): GetEnable（读 group enable 状态）。
    fn GetEnable(&self) -> Result<BOOL> {
        nyi()
    }
}

impl IConnectionPointContainer_Impl for GroupObj_Impl {
    // TODO(后续阶段): EnumConnectionPoints（枚举 Group 的 cp：IOPCDataCallback）。
    fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
        nyi()
    }

    fn FindConnectionPoint(&self, riid: *const GUID) -> Result<IConnectionPoint> {
        if riid.is_null() {
            return Err(Error::from(E_POINTER));
        }
        // SAFETY: riid 非空（上面校验）；调用方提供的有效 GUID。
        let iid = unsafe { *riid };
        if iid == <IOPCDataCallback as Interface>::IID {
            // data_cp 是 IConnectionPoint 接口；clone = AddRef，返回独立指针指向同一
            // ConnectionPoint 对象，所有权交 client（client Unadvise/Release 时减 ref）。
            Ok(self.data_cp.clone())
        } else {
            Err(Error::from(E_NOINTERFACE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_source::{OPC_QUALITY_GOOD, SimDataSource};
    use opc_da_client::bindings::comn::IOPCShutdown;
    use opc_da_client::bindings::da::{
        IOPCAsyncIO2, IOPCDataCallback, IOPCDataCallback_Impl, IOPCItemMgt, IOPCSyncIO,
        OPC_DS_DEVICE,
    };
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Variant::{VARIANT, VT_I4, VT_R8};
    use windows::core::{IUnknown, Interface};

    /// 构造测试用 `OPCITEMDEF`（`szItemID` 借用 `wide` 的内存，`wide` 须在 AddItems 期间存活）。
    fn make_def(wide: &[u16], h_client: u32, active: bool) -> tagOPCITEMDEF {
        tagOPCITEMDEF {
            szAccessPath: PWSTR(core::ptr::null_mut()),
            szItemID: PWSTR(wide.as_ptr().cast_mut()),
            bActive: BOOL::from(active),
            hClient: h_client,
            dwBlobSize: 0,
            pBlob: core::ptr::null_mut(),
            vtRequestedDataType: 0, // 0 = 让 server 决定 canonical 类型
            wReserved: 0,
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn new_group() -> IOPCItemMgt {
        let ds: Arc<dyn DataSource> = Arc::new(SimDataSource::new());
        GroupObj::new(ds, String::new(), true, 1000, 0, 0.0, 0, 0, 0).into()
    }

    /// AddItems 核心：有效 item 注册成功，返回非 0 hServer + canonical 类型 + S_OK。
    #[test]
    fn add_items_valid_returns_handle() {
        let g = new_group();
        let id = wide("Random.Int4");
        let defs = [make_def(&id, 100, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: g 是同进程 implement 对象；defs/results/errors 指针有效。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut errors)
                .expect("AddItems");
        }
        // SAFETY: AddItems 成功写入 results[0] / errors[0]。
        unsafe {
            let r = &*results;
            assert_ne!(r.hServer, 0, "有效 item 应得非 0 hServer");
            assert_eq!(
                r.vtCanonicalDataType, VT_I4.0,
                "Random.Int4 canonical=VT_I4"
            );
            assert_eq!(*errors, S_OK);
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errors.cast()));
        }
    }

    /// AddItems 核心：未知 item 拒收（OPC_E_INVALIDITEMID）。
    #[test]
    fn add_items_unknown_rejected() {
        let g = new_group();
        let id = wide("nope.nope");
        let defs = [make_def(&id, 1, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut errors)
                .expect("AddItems 调用本身成功");
        }
        // SAFETY: AddItems 写入 errors[0]（即使 item 无效）。
        unsafe {
            assert_eq!(*errors, OPC_E_INVALIDITEMID, "未知 item 应 INVALIDITEMID");
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errors.cast()));
        }
    }

    /// 0-item 请求（client 握手/测试常见）：必须返回 Ok 且 out=null——绝不能留垃圾指针让
    /// client 读（Prosys 实测：0-item Read 若返回 E_POINTER 且不写 out，client 读未初始化
    /// out → 进程内 access violation）。Read / AddItems / RemoveItems 三路径全覆盖。
    #[test]
    fn zero_count_requests_succeed_with_null_out() {
        let g = new_group();

        // Read(0) → S_OK + ppitemvalues=null + pperrors=null。
        // 先把 out 预置为垃圾指针（模拟 client 未初始化栈变量），验证 server 一定覆盖为 null。
        // SAFETY: 同进程 implement 对象同 CLSID 内 QI 必然成功；cast 失败 expect 不 panic（测试）。
        let sync: IOPCSyncIO = g.cast().expect("QI IOPCSyncIO");
        let mut values: *mut tagOPCITEMSTATE = 0x1000usize as *mut tagOPCITEMSTATE;
        let mut errors: *mut HRESULT = 0x1000usize as *mut HRESULT;
        // SAFETY: sync 是同进程 implement 对象；out 指针有效。
        unsafe {
            sync.Read(
                OPC_DS_DEVICE,
                0,
                core::ptr::null_mut(),
                &raw mut values,
                &raw mut errors,
            )
            .expect("0-item Read 应成功");
        }
        assert!(values.is_null(), "0-item Read ppitemvalues 必须 null");
        assert!(errors.is_null(), "0-item Read pperrors 必须 null");

        // AddItems(0) → S_OK + out null。
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors2: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.AddItems(0, core::ptr::null_mut(), &raw mut results, &raw mut errors2)
                .expect("0-item AddItems 应成功");
        }
        assert!(results.is_null(), "0-item AddItems 结果必须 null");
        assert!(errors2.is_null(), "0-item AddItems errors 必须 null");

        // RemoveItems(0) → S_OK + out null。
        let mut errors3: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.RemoveItems(0, core::ptr::null_mut(), &raw mut errors3)
                .expect("0-item RemoveItems 应成功");
        }
        assert!(errors3.is_null(), "0-item RemoveItems errors 必须 null");
    }

    /// 5 接口 QI 共存仍成立（重构后不破坏）。
    #[test]
    fn multi_interface_qi_succeeds() {
        let ds: Arc<dyn DataSource> = Arc::new(SimDataSource::new());
        let obj: IUnknown = GroupObj::new(ds, String::new(), true, 1000, 0, 0.0, 0, 0, 0).into();
        use opc_da_client::bindings::da::IOPCGroupStateMgt;
        use windows::Win32::System::Com::IConnectionPointContainer;
        assert!(obj.cast::<IOPCItemMgt>().is_ok());
        assert!(obj.cast::<IOPCGroupStateMgt>().is_ok());
        assert!(obj.cast::<IOPCSyncIO>().is_ok());
        assert!(obj.cast::<IOPCAsyncIO2>().is_ok());
        assert!(obj.cast::<IConnectionPointContainer>().is_ok());
    }

    /// RemoveItems 核心：已注册 item 可移除（S_OK），再移除同 handle 失败（E_INVALIDARG）。
    #[test]
    fn remove_items_after_add() {
        let g = new_group();
        let id = wide("Bucket Brigade.Int4");
        let defs = [make_def(&id, 1, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同 add 测试。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut errors)
                .expect("AddItems");
            let h_server = (*results).hServer;
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errors.cast()));

            // 第一次移除：S_OK。
            let handles = [h_server];
            let mut errs1: *mut HRESULT = core::ptr::null_mut();
            g.RemoveItems(1, handles.as_ptr(), &raw mut errs1)
                .expect("RemoveItems");
            assert_eq!(*errs1, S_OK, "已注册 item 移除应 S_OK");
            CoTaskMemFree(Some(errs1.cast()));

            // 再移除同 handle：E_INVALIDARG（已不存在）。
            let mut errs2: *mut HRESULT = core::ptr::null_mut();
            g.RemoveItems(1, handles.as_ptr(), &raw mut errs2)
                .expect("RemoveItems 2");
            assert_eq!(*errs2, E_INVALIDARG, "已移除 item 再移除应 E_INVALIDARG");
            CoTaskMemFree(Some(errs2.cast()));
        }
    }

    /// SetActiveState 核心：改 active 状态（S_OK）；未知 handle 拒绝。
    #[test]
    fn set_active_state_toggles() {
        let g = new_group();
        let id = wide("Random.Real8");
        let defs = [make_def(&id, 5, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut errors)
                .expect("AddItems");
            let h = (*results).hServer;
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errors.cast()));

            let handles = [h];
            let mut errs: *mut HRESULT = core::ptr::null_mut();
            g.SetActiveState(1, handles.as_ptr(), false, &raw mut errs)
                .expect("SetActiveState");
            assert_eq!(*errs, S_OK, "已知 handle 改 active 应 S_OK");
            CoTaskMemFree(Some(errs.cast()));

            let mut errs_bad: *mut HRESULT = core::ptr::null_mut();
            g.SetActiveState(1, [9999u32].as_ptr(), true, &raw mut errs_bad)
                .expect("SetActiveState bad");
            assert_eq!(*errs_bad, E_INVALIDARG, "未知 handle 应 E_INVALIDARG");
            CoTaskMemFree(Some(errs_bad.cast()));
        }
    }

    /// SetClientHandles 核心：改 hClient（S_OK）。
    #[test]
    fn set_client_handles_updates() {
        let g = new_group();
        let id = wide("Square Waves.Real8");
        let defs = [make_def(&id, 1, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errors: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut errors)
                .expect("AddItems");
            let h = (*results).hServer;
            assert_eq!((*results).vtCanonicalDataType, VT_R8.0);
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errors.cast()));

            let handles = [h];
            let clients = [777u32];
            let mut errs: *mut HRESULT = core::ptr::null_mut();
            g.SetClientHandles(1, handles.as_ptr(), clients.as_ptr(), &raw mut errs)
                .expect("SetClientHandles");
            assert_eq!(*errs, S_OK);
            CoTaskMemFree(Some(errs.cast()));
        }
    }

    /// 构造 `VT_I4` VARIANT（测试用，集中 unsafe；镜像 `data_source::variant_i4`）。
    fn variant_i4(value: i32) -> VARIANT {
        let mut var = VARIANT::default();
        // SAFETY: 设 vt 判别 + lVal 字段；var 按值返回，无并发/别名。
        unsafe {
            (*var.Anonymous.Anonymous).vt = VT_I4;
            (*var.Anonymous.Anonymous).Anonymous.lVal = value;
        }
        var
    }

    /// `IOPCSyncIO::Read` 核心：已注册 item 读出正确类型 + GOOD quality + hClient 回传。
    #[test]
    fn sync_read_returns_value_quality_and_client_handle() {
        let g = new_group();
        let id = wide("Random.Int4");
        let defs = [make_def(&id, 100, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut add_errs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: g 同进程 implement；指针有效。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut add_errs)
                .expect("AddItems");
        }
        let h_server;
        // SAFETY: AddItems 成功写入 results[0]。
        unsafe {
            h_server = (*results).hServer;
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(add_errs.cast()));
        }
        // 同一对象 QI 到 IOPCSyncIO（多接口 implement）。
        let sync: IOPCSyncIO = g.cast::<IOPCSyncIO>().expect("QI IOPCSyncIO");
        let handles = [h_server];
        let mut states: *mut tagOPCITEMSTATE = core::ptr::null_mut();
        let mut errs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: sync 同进程；handles/states/errs 指针有效。
        unsafe {
            sync.Read(
                OPC_DS_DEVICE,
                1,
                handles.as_ptr(),
                &raw mut states,
                &raw mut errs,
            )
            .expect("Read");
        }
        // SAFETY: Read 成功写入 states[0]/errs[0]；vDataValue 是 VT_I4 纯标量，按位读安全。
        unsafe {
            let s = &*states;
            assert_eq!(*errs, S_OK, "Read 已注册 item 应 S_OK");
            assert_eq!(s.hClient, 100, "hClient 回传 AddItems 时的值");
            assert_eq!(s.wQuality, OPC_QUALITY_GOOD, "quality GOOD");
            assert_eq!(
                (*s.vDataValue.Anonymous.Anonymous).vt,
                VT_I4,
                "vDataValue 应 VT_I4"
            );
            CoTaskMemFree(Some(states.cast()));
            CoTaskMemFree(Some(errs.cast()));
        }
    }

    /// `IOPCSyncIO::Read` 核心：未知 handle 返回 E_INVALIDARG（不 panic）。
    #[test]
    fn sync_read_unknown_handle_is_invalid_arg() {
        let g = new_group();
        let sync: IOPCSyncIO = g.cast::<IOPCSyncIO>().expect("QI IOPCSyncIO");
        let handles = [9999u32];
        let mut states: *mut tagOPCITEMSTATE = core::ptr::null_mut();
        let mut errs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: sync 同进程；指针有效。
        unsafe {
            sync.Read(
                OPC_DS_DEVICE,
                1,
                handles.as_ptr(),
                &raw mut states,
                &raw mut errs,
            )
            .expect("Read 调用本身成功");
        }
        // SAFETY: Read 写入 errs[0]（即使 handle 无效）。
        unsafe {
            assert_eq!(*errs, E_INVALIDARG, "未知 handle 应 E_INVALIDARG");
            CoTaskMemFree(Some(states.cast()));
            CoTaskMemFree(Some(errs.cast()));
        }
    }

    /// `IOPCSyncIO::Write` + `Read` round-trip 核心：写可写 tag 后读反映写入值。
    #[test]
    fn sync_write_then_read_round_trip() {
        let g = new_group();
        let id = wide("Bucket Brigade.Int4");
        let defs = [make_def(&id, 1, true)];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut add_errs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: 同上。
        unsafe {
            g.AddItems(1, defs.as_ptr(), &raw mut results, &raw mut add_errs)
                .expect("AddItems");
        }
        let h_server;
        // SAFETY: AddItems 成功。
        unsafe {
            h_server = (*results).hServer;
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(add_errs.cast()));
        }
        let sync: IOPCSyncIO = g.cast::<IOPCSyncIO>().expect("QI IOPCSyncIO");
        let handles = [h_server];

        // Write 42。
        let write_val = variant_i4(42);
        let mut werrs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: sync 同进程；write_val/handles/werrs 有效。
        unsafe {
            sync.Write(1, handles.as_ptr(), &raw const write_val, &raw mut werrs)
                .expect("Write");
        }
        // SAFETY: Write 写入 werrs[0]。
        unsafe {
            assert_eq!(*werrs, S_OK, "写 Bucket Brigade 应 S_OK");
            CoTaskMemFree(Some(werrs.cast()));
        }

        // Read 回 → 应为 42（round trip）。
        let mut states: *mut tagOPCITEMSTATE = core::ptr::null_mut();
        let mut rerrs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: sync 同进程；指针有效。
        unsafe {
            sync.Read(
                OPC_DS_DEVICE,
                1,
                handles.as_ptr(),
                &raw mut states,
                &raw mut rerrs,
            )
            .expect("Read");
        }
        // SAFETY: Read 写入 states[0]/rerrs[0]；vDataValue VT_I4 标量，按位读安全。
        unsafe {
            let s = &*states;
            assert_eq!(*rerrs, S_OK);
            assert_eq!(
                (*s.vDataValue.Anonymous.Anonymous).vt,
                VT_I4,
                "read 回应 VT_I4"
            );
            assert_eq!(
                (*s.vDataValue.Anonymous.Anonymous).Anonymous.lVal,
                42,
                "read 应反映写入的 42（round trip）"
            );
            CoTaskMemFree(Some(states.cast()));
            CoTaskMemFree(Some(rerrs.cast()));
        }
    }

    /// `IOPCGroupStateMgt::SetState` + `GetState` round-trip 核心：SetState 改 update_rate/
    /// active 后，GetState 反映新值（其余 in 指针 null = 不改）。
    #[test]
    fn set_state_then_get_state_reflects() {
        let g = new_group(); // 初值 update_rate=1000, active=true
        let mgt: IOPCGroupStateMgt = g.cast::<IOPCGroupStateMgt>().expect("QI IOPCGroupStateMgt");
        let requested_rate = 250u32;
        let active_false = BOOL::from(false);
        let mut revised = 0u32;
        // SAFETY: mgt 同进程；requested_rate/active_false/out 指针有效；其余 in 指针 null=不改。
        unsafe {
            mgt.SetState(
                &raw const requested_rate,
                &raw mut revised,
                &raw const active_false,
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
            )
            .expect("SetState");
        }
        assert_eq!(revised, 250, "revised = requested（无最小 rate 约束）");

        // GetState 验证 update_rate/active 已改。
        let mut rate = 0u32;
        let mut active = BOOL::default();
        let mut name = PWSTR::null();
        let mut tb = 0i32;
        let mut pd = 0.0f32;
        let mut lcid = 0u32;
        let mut hc = 0u32;
        let mut hs = 0u32;
        // SAFETY: mgt 同进程；8 个 out 指针有效。
        unsafe {
            mgt.GetState(
                &raw mut rate,
                &raw mut active,
                &raw mut name,
                &raw mut tb,
                &raw mut pd,
                &raw mut lcid,
                &raw mut hc,
                &raw mut hs,
            )
            .expect("GetState");
        }
        assert_eq!(rate, 250, "GetState update_rate 反映 SetState");
        assert!(!active.as_bool(), "GetState active=false 反映 SetState");
        // SAFETY: name 由 GetState CoTaskMemAlloc 分配，调用方释放。
        unsafe {
            CoTaskMemFree(Some(name.as_ptr() as *const _));
        }
    }

    /// `IOPCGroupStateMgt::SetName` + `GetState` 核心：SetName 后 GetState 的 name 反映。
    #[test]
    fn set_name_then_get_state_reflects() {
        let g = new_group(); // 初值 name=""
        let mgt: IOPCGroupStateMgt = g.cast::<IOPCGroupStateMgt>().expect("QI IOPCGroupStateMgt");
        let name_wide = wide("my-group");
        // SAFETY: mgt 同进程；PCWSTR 借用 name_wide（0 结尾 UTF-16）。
        unsafe {
            mgt.SetName(PCWSTR(name_wide.as_ptr())).expect("SetName");
        }
        let mut rate = 0u32;
        let mut active = BOOL::default();
        let mut name = PWSTR::null();
        let mut tb = 0i32;
        let mut pd = 0.0f32;
        let mut lcid = 0u32;
        let mut hc = 0u32;
        let mut hs = 0u32;
        // SAFETY: 同上。
        unsafe {
            mgt.GetState(
                &raw mut rate,
                &raw mut active,
                &raw mut name,
                &raw mut tb,
                &raw mut pd,
                &raw mut lcid,
                &raw mut hc,
                &raw mut hs,
            )
            .expect("GetState");
        }
        assert_eq!(
            pwstr_to_string(name),
            "my-group",
            "SetName 后 GetState name 反映"
        );
        // SAFETY: name CoTaskMemAlloc 分配。
        unsafe {
            CoTaskMemFree(Some(name.as_ptr() as *const _));
        }
    }

    /// `IConnectionPointContainer::FindConnectionPoint(IOPCDataCallback::IID)` 返回 data_cp
    /// 的 IConnectionPoint，其 `GetConnectionInterface` == IOPCDataCallback::IID。
    #[test]
    fn find_connection_point_returns_data_cp_for_datacallback() {
        let g = new_group();
        let cpc: IConnectionPointContainer = g
            .cast::<IConnectionPointContainer>()
            .expect("QI IConnectionPointContainer");
        // SAFETY: cpc 同进程 implement 对象；iid_dc 有效 GUID。
        let iid_dc = IOPCDataCallback::IID;
        let cp = unsafe { cpc.FindConnectionPoint(&raw const iid_dc) }
            .expect("FindConnectionPoint IOPCDataCallback");
        // SAFETY: cp 同进程；GetConnectionInterface 返回 cp 注册的 sink IID。
        let iid = unsafe { cp.GetConnectionInterface() }.expect("GetConnectionInterface");
        assert_eq!(
            iid,
            IOPCDataCallback::IID,
            "data_cp 的连接接口应是 IOPCDataCallback"
        );
    }

    /// `FindConnectionPoint` 不支持的 sink IID（IOPCShutdown）返回 E_NOINTERFACE。
    #[test]
    fn find_connection_point_unsupported_iid_returns_nointerface() {
        let g = new_group();
        let cpc: IConnectionPointContainer = g
            .cast::<IConnectionPointContainer>()
            .expect("QI IConnectionPointContainer");
        // SAFETY: cpc 同进程。
        let iid_shutdown = IOPCShutdown::IID;
        let err = unsafe { cpc.FindConnectionPoint(&raw const iid_shutdown) }.unwrap_err();
        assert_eq!(
            err.code(),
            E_NOINTERFACE,
            "Group 不支持 IOPCShutdown（那是 Server 的 cp），应 E_NOINTERFACE"
        );
    }

    /// 测试用 `IOPCDataCallback` sink：捕获 `OnDataChange` 的 `(trans_id, h_group, count)`，
    /// 经共享 `Arc<Mutex>` 让测试同步读取（同进程 COM 调用直接进 OnDataChange）。
    #[implement(IOPCDataCallback)]
    struct CapturingSink {
        last: Arc<Mutex<Option<(u32, u32, u32)>>>,
    }

    // IOPCDataCallback 4 方法：OnDataChange 记录；其余 no-op（Refresh2 只触发 OnDataChange）。
    #[allow(clippy::too_many_arguments)] // COM vtable 签名固定（bindings 生成）
    impl IOPCDataCallback_Impl for CapturingSink_Impl {
        fn OnDataChange(
            &self,
            dwtransid: u32,
            hgroup: u32,
            _hrmasterquality: HRESULT,
            _hrmastererror: HRESULT,
            dwcount: u32,
            _phclientitems: *const u32,
            _pvvalues: *const VARIANT,
            _pwqualities: *const u16,
            _pfttimestamps: *const FILETIME,
            _perrors: *const HRESULT,
        ) -> Result<()> {
            *locked(&self.last) = Some((dwtransid, hgroup, dwcount));
            Ok(())
        }

        fn OnReadComplete(
            &self,
            _dwtransid: u32,
            _hgroup: u32,
            _hrmasterquality: HRESULT,
            _hrmastererror: HRESULT,
            _dwcount: u32,
            _phclientitems: *const u32,
            _pvvalues: *const VARIANT,
            _pwqualities: *const u16,
            _pfttimestamps: *const FILETIME,
            _perrors: *const HRESULT,
        ) -> Result<()> {
            Ok(())
        }

        fn OnWriteComplete(
            &self,
            _dwtransid: u32,
            _hgroup: u32,
            _hrmastererr: HRESULT,
            _dwcount: u32,
            _pclienthandles: *const u32,
            _perrors: *const HRESULT,
        ) -> Result<()> {
            Ok(())
        }

        fn OnCancelComplete(&self, _dwtransid: u32, _hgroup: u32) -> Result<()> {
            Ok(())
        }
    }

    /// `GroupInner::snapshot_active_for_publish`：只含 active items（inactive 过滤）。
    #[test]
    fn snapshot_active_for_publish_filters_inactive() {
        let mut inner = GroupInner {
            items: HashMap::new(),
            next_handle: 1,
            update_rate: 500,
            active: true,
            name: "test".into(),
            time_bias: 0,
            percent_deadband: 0.0,
            locale: 0,
            h_client_group: 42,
            h_server_group: 7,
        };
        inner.items.insert(
            1,
            ItemEntry {
                item_id: Arc::from("Random.Int4"),
                h_client: 100,
                active: true,
                data_type: VT_I4,
                last_pushed: None,
            },
        );
        inner.items.insert(
            2,
            ItemEntry {
                item_id: Arc::from("Random.Real8"),
                h_client: 200,
                active: false,
                data_type: VT_R8,
                last_pushed: None,
            },
        );
        let (h_group, frames) = inner.snapshot_active_for_publish();
        assert_eq!(h_group, 42, "h_client_group 回传");
        assert_eq!(frames.len(), 1, "只 1 个 active item（inactive 过滤）");
        assert_eq!(
            frames[0],
            (100, Arc::from("Random.Int4")),
            "active item 的 h_client + item_id"
        );
    }

    /// `IOPCAsyncIO2::Refresh2` 核心：触发一次 `OnDataChange`，带 client 的 dwTransactionID，
    /// 且只推 active items。验证 Refresh2 意图——client 主动刷新拿当前全量快照。
    #[test]
    fn refresh2_pushes_ondatachange_with_transaction_id() {
        use windows::Win32::System::Com::{IConnectionPoint, IConnectionPointContainer};

        let g = new_group();
        // AddItems：1 active + 1 inactive（验证 Refresh 只推 active）。
        let id_active = wide("Random.Int4");
        let id_inactive = wide("Random.Real8");
        let defs = [
            make_def(&id_active, 100, true),
            make_def(&id_inactive, 200, false),
        ];
        let mut results: *mut tagOPCITEMRESULT = core::ptr::null_mut();
        let mut errs: *mut HRESULT = core::ptr::null_mut();
        // SAFETY: g 同进程 implement；defs/results/errs 指针有效。
        unsafe {
            g.AddItems(2, defs.as_ptr(), &raw mut results, &raw mut errs)
                .expect("AddItems");
            CoTaskMemFree(Some(results.cast()));
            CoTaskMemFree(Some(errs.cast()));
        }

        // Advise 捕获 sink（共享 Arc 让测试读 OnDataChange 记录）。
        let last: Arc<Mutex<Option<(u32, u32, u32)>>> = Arc::new(Mutex::new(None));
        let sink: IOPCDataCallback = CapturingSink {
            last: Arc::clone(&last),
        }
        .into();
        let cpc: IConnectionPointContainer = g.cast::<IConnectionPointContainer>().expect("QI CPC");
        let iid_dc = IOPCDataCallback::IID;
        // SAFETY: cpc 同进程；iid_dc 有效 GUID。
        let cp: IConnectionPoint =
            unsafe { cpc.FindConnectionPoint(&raw const iid_dc) }.expect("FindConnectionPoint");
        let sink_unk: IUnknown = sink.cast::<IUnknown>().expect("cast IUnknown");
        // SAFETY: cp 同进程；sink_unk 有效 IUnknown。返回 cookie（测试不验证，仅确认 Advise 成功）。
        let _cookie = unsafe { cp.Advise(&sink_unk) }.expect("Advise");

        // Refresh2(dwTransactionID=12345)——同步推送（返回前 OnDataChange 已调）。
        let async2: IOPCAsyncIO2 = g.cast::<IOPCAsyncIO2>().expect("QI AsyncIO2");
        // SAFETY: async2 同进程；Refresh2 内同步调 publisher::push_data_change → sink.OnDataChange。
        let cancel_id = unsafe { async2.Refresh2(OPC_DS_DEVICE, 12345) }.expect("Refresh2");
        assert!(cancel_id >= 1, "cancel id 应非 0（从 1 起）");

        // 验证 sink 收到 OnDataChange(12345, count=1 active only)。
        let captured = *locked(&last);
        let (tid, _hgroup, count) = captured.expect("Refresh2 应触发 OnDataChange");
        assert_eq!(tid, 12345, "dwTransactionID 应透传到 OnDataChange");
        assert_eq!(count, 1, "只推 1 个 active item（inactive 过滤）");
    }

    /// `Scheduler` register/unregister 计数（P0）：注册增、注销减、跨 rate 桶、幂等。
    #[test]
    fn scheduler_register_unregister_updates_count() {
        use crate::objects::scheduler::Scheduler;
        let s = Scheduler::new();
        assert_eq!(s.registered_count(), 0, "初始 0");
        let ds: Arc<dyn DataSource> = Arc::new(SimDataSource::new());
        let inner = Arc::new(Mutex::new(GroupInner {
            items: HashMap::new(),
            next_handle: 1,
            update_rate: 100,
            active: true,
            name: "t".into(),
            time_bias: 0,
            percent_deadband: 0.0,
            locale: 0,
            h_client_group: 0,
            h_server_group: 1,
        }));
        let cp = ConnectionPoint::<IOPCDataCallback>::new();
        let data_sinks = cp.sinks_arc();
        s.register(
            1,
            Arc::clone(&inner),
            Arc::clone(&ds),
            data_sinks.clone(),
            100,
        );
        assert_eq!(s.registered_count(), 1, "注册 1 个后 1");
        // 不同 rate → 进不同桶（验证多桶注册/注销）。ds/data_sinks 最后一次用，move 避免 redundant clone。
        s.register(2, Arc::clone(&inner), ds, data_sinks, 250);
        assert_eq!(s.registered_count(), 2, "注册第 2 个（不同 rate）后 2");
        s.unregister(1);
        assert_eq!(s.registered_count(), 1, "注销 1 后 1");
        s.unregister(2);
        assert_eq!(s.registered_count(), 0, "全注销后 0");
        s.unregister(999); // 幂等：不存在的 key no-op。
        assert_eq!(s.registered_count(), 0, "注销不存在 key 幂等");
    }
}
