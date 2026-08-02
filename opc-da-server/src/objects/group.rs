//! Group 对象——阶段 1。
//!
//! [`GroupObj`] 实现 `IOPCItemMgt` + 4 个仍为骨架（`E_NOTIMPL`）的接口
//!（`IOPCGroupStateMgt` / `IOPCSyncIO` / `IOPCAsyncIO2` / `IConnectionPointContainer`）。
//! `IOPCItemMgt` 已真实实装：`AddItems` / `ValidateItems` / `RemoveItems` /
//! `SetActiveState` / `SetClientHandles`——item 注册表（[`GroupInner`]）+ DataSource
//! 元数据查询 + COM 内存（`CoTaskMemAlloc` 数组所有权交 client）。
//!
//! `SetDatatypes` / `CreateEnumerator` 暂 `E_NOTIMPL`（后续阶段）。

// `#[implement]` 展开的 COM 胶水（`_Impl`/`_Vtbl`）触发若干 pedantic lints；与
// `class_factory.rs` / `server.rs` 同模式 allow。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, S_OK};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, CoTaskMemFree, IConnectionPoint, IConnectionPointContainer,
    IConnectionPointContainer_Impl, IEnumConnectionPoints,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT};
use windows::core::{BOOL, Error, GUID, HRESULT, IUnknown, PCWSTR, PWSTR, Result, implement};

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
fn pwstr_to_string(p: PWSTR) -> String {
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
#[allow(dead_code)] // data_type 待 publisher/IOPCItemProperties 用（Read 经 DataSource 按 item_id 取值）
struct ItemEntry {
    item_id: String,
    h_client: u32,
    active: bool,
    data_type: VARENUM,
}

/// Group 的可变状态（item 注册表 + server handle 分配器）。`Mutex` 守护，跨 COM 调用线程。
struct GroupInner {
    items: HashMap<u32, ItemEntry>,
    next_handle: u32,
}

impl GroupInner {
    #[allow(dead_code)] // GroupObj::new 调用；后者待 IOPCServer::AddGroup 接入
    fn new() -> Self {
        Self {
            items: HashMap::new(),
            next_handle: 1,
        }
    }

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
    inner: Mutex<GroupInner>,
    data_source: Arc<dyn DataSource>,
    /// 订阅 sink 连接点。后续 `IConnectionPointContainer::FindConnectionPoint` 返回它，
    /// publisher 引擎（§10）遍历推送 `OnDataChange`。
    #[allow(dead_code)] // 后续 FindConnectionPoint + publisher 接入
    pub(crate) data_cp: ConnectionPoint<IOPCDataCallback>,
}

impl GroupObj {
    /// 新建 Group（空 item 注册表，绑定 DataSource）。
    #[allow(dead_code)] // 待 IOPCServer::AddGroup 接入（当前仅测试构造）
    pub(crate) fn new(data_source: Arc<dyn DataSource>) -> Self {
        Self {
            inner: Mutex::new(GroupInner::new()),
            data_source,
            data_cp: ConnectionPoint::new(),
        }
    }
}

/// 通用"未实装"错误：尚未实装的方法返回 `E_NOTIMPL`。
fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
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
        let mut inner = locked(&self.inner);
        for i in 0..n {
            // SAFETY: pitemarray 含 dwcount 个 OPCITEMDEF（调用方保证）；i < n = dwcount。
            let def = unsafe { &*pitemarray.add(i) };
            let item_id = pwstr_to_string(def.szItemID);
            match self.data_source.item_meta(&item_id) {
                Some(meta) => {
                    let h_server = inner.alloc_handle();
                    inner.items.insert(
                        h_server,
                        ItemEntry {
                            item_id,
                            h_client: def.hClient,
                            active: def.bActive.as_bool(),
                            data_type: meta.data_type,
                        },
                    );
                    // SAFETY: results_ptr 含 n 个槽；i < n。
                    unsafe {
                        *(*ppaddresults).add(i) = tagOPCITEMRESULT {
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
                        };
                        *(*pperrors).add(i) = S_OK;
                    }
                }
                None => {
                    // SAFETY: 同上；零结果 + INVALIDITEMID。
                    unsafe {
                        *(*ppaddresults).add(i) = tagOPCITEMRESULT {
                            hServer: 0,
                            vtCanonicalDataType: 0,
                            wReserved: 0,
                            dwAccessRights: 0,
                            dwBlobSize: 0,
                            pBlob: core::ptr::null_mut(),
                        };
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
                    // SAFETY: ppvalidationresults 含 n 个槽；i < n。
                    unsafe {
                        *(*ppvalidationresults).add(i) = tagOPCITEMRESULT {
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
                        };
                        *(*pperrors).add(i) = S_OK;
                    }
                }
                None => {
                    // SAFETY: 同上。
                    unsafe {
                        *(*ppvalidationresults).add(i) = tagOPCITEMRESULT {
                            hServer: 0,
                            vtCanonicalDataType: 0,
                            wReserved: 0,
                            dwAccessRights: 0,
                            dwBlobSize: 0,
                            pBlob: core::ptr::null_mut(),
                        };
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

    fn SetDatatypes(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _prequesteddatatypes: *const u16,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

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
    if dwcount == 0 || pitemarray.is_null() || poutresults.is_null() || pperrors.is_null() {
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
    if dwcount == 0 || phserver.is_null() || pperrors.is_null() {
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
    if dwcount == 0 || phserver.is_null() || ppitemvalues.is_null() || pperrors.is_null() {
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
        _pupdaterate: *mut u32,
        _pactive: *mut BOOL,
        _ppname: *mut PWSTR,
        _ptimebias: *mut i32,
        _ppercentdeadband: *mut f32,
        _plcid: *mut u32,
        _phclientgroup: *mut u32,
        _phservergroup: *mut u32,
    ) -> Result<()> {
        nyi()
    }

    fn SetState(
        &self,
        _prequestedupdaterate: *const u32,
        _previsedupdaterate: *mut u32,
        _pactive: *const BOOL,
        _ptimebias: *const i32,
        _ppercentdeadband: *const f32,
        _plcid: *const u32,
        _phclientgroup: *const u32,
    ) -> Result<()> {
        nyi()
    }

    fn SetName(&self, _szname: &PCWSTR) -> Result<()> {
        nyi()
    }

    fn CloneGroup(&self, _szname: &PCWSTR, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }
}

impl IOPCSyncIO_Impl for GroupObj_Impl {
    fn Read(
        &self,
        _dwsource: tagOPCDATASOURCE,
        dwcount: u32,
        phserver: *const u32,
        ppitemvalues: *mut *mut tagOPCITEMSTATE,
        pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        // dwsource 忽略：SimDataSource 无 cache/device 之分，统一 read-time 现算
        //（等价 device 读）。真 cache 语义待 publisher 引擎（§10）维护缓存后区分。
        let n = prealloc_item_states(dwcount, phserver, ppitemvalues, pperrors)?;
        // 锁内取每个 handle 对应的 (hClient, item_id)；未知 handle 记 None。
        // 短持锁：仅查表 clone，DataSource::read 在锁外执行（避免长持锁期间做 IO）。
        let lookups: Vec<(u32, Option<String>)> = {
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
            // SAFETY: ppitemvalues / pperrors 各含 n 个槽（prealloc 分配）；i < n。
            // 直接赋值覆盖未初始化槽（move VARIANT 进去，不 drop 旧值——裸内存）。
            unsafe {
                match maybe_id {
                    Some(item_id) => {
                        let (v, q, ts) = self.data_source.read(&item_id);
                        *(*ppitemvalues).add(i) = tagOPCITEMSTATE {
                            hClient: h_client,
                            ftTimeStamp: ts,
                            wQuality: q,
                            wReserved: 0,
                            vDataValue: v,
                        };
                        *(*pperrors).add(i) = S_OK;
                    }
                    None => {
                        // 未知 handle：空 ITEMSTATE（vDataValue=VT_EMPTY）+ E_INVALIDARG。
                        *(*ppitemvalues).add(i) = tagOPCITEMSTATE::default();
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
        let lookups: Vec<Option<String>> = {
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

    fn Refresh2(&self, _dwsource: tagOPCDATASOURCE, _dwtransactionid: u32) -> Result<u32> {
        nyi()
    }

    fn Cancel2(&self, _dwcancelid: u32) -> Result<()> {
        nyi()
    }

    fn SetEnable(&self, _benable: BOOL) -> Result<()> {
        nyi()
    }

    fn GetEnable(&self) -> Result<BOOL> {
        nyi()
    }
}

impl IConnectionPointContainer_Impl for GroupObj_Impl {
    fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
        nyi()
    }

    fn FindConnectionPoint(&self, _riid: *const GUID) -> Result<IConnectionPoint> {
        nyi()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_source::{OPC_QUALITY_GOOD, SimDataSource};
    use opc_da_client::bindings::da::{IOPCItemMgt, IOPCSyncIO, OPC_DS_DEVICE};
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
        GroupObj::new(ds).into()
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

    /// 5 接口 QI 共存仍成立（重构后不破坏）。
    #[test]
    fn multi_interface_qi_succeeds() {
        let ds: Arc<dyn DataSource> = Arc::new(SimDataSource::new());
        let obj: IUnknown = GroupObj::new(ds).into();
        use opc_da_client::bindings::da::{IOPCAsyncIO2, IOPCGroupStateMgt, IOPCSyncIO};
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
}
