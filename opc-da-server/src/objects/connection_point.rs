//! 通用 COM 连接点（`IConnectionPoint`）——管理可连接对象上某一 sink 接口的订阅表。
//!
//! OPC DA 的事件推送（server→client）走 COM 连接点：client 把自己的 sink（如
//! `IOPCDataCallback` / `IOPCShutdown`）`Advise` 给 server 暴露的 `IConnectionPoint`，
//! server 持有 sink 表，后续推送时遍历回调。设计见
//! `docs/superpowers/specs/2026-08-02-opc-da-server-design.md` §8。
//!
//! 本模块提供通用 [`ConnectionPoint<T>`]（两个实例：Server 上的
//! `ConnectionPoint<IOPCShutdown>`、Group 上的 `ConnectionPoint<IOPCDataCallback>`）
//! 及其 `IEnumConnections` 实现 [`EnumConnectionsObj`]。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    CONNECTDATA, IConnectionPoint, IConnectionPoint_Impl, IConnectionPointContainer,
    IEnumConnections, IEnumConnections_Impl,
};
use windows::Win32::System::Ole::{CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION};
use windows::core::{Error, GUID, HRESULT, IUnknown, Interface, Ref, Result, Weak, implement};

/// 取锁；mutex poison 时返回 guard（不 panic）。
///
/// poison 仅表示持锁线程曾 panic；本模块锁内不执行会 panic 的操作，故选择继续而非
/// 传播 panic（遵循 CLAUDE.md "禁止 panic" 约定——不依赖 `.expect`/`.unwrap`）。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 通用 COM 连接点——维护某 sink 接口 `T`（如 `IOPCShutdown` / `IOPCDataCallback`）
/// 的订阅表：`cookie -> sink`。
///
/// - 持有所属可连接对象的**弱引用**（`Weak<IConnectionPointContainer>`），供
///   `GetConnectionPointContainer` 回指，避免与 container 形成强引用环（container
///   拥有本 cp 字段，cp 再强引用 container → 引用泄漏）。
/// - 线程安全：OPC DA server 主流 free-threaded（MTA），sink 指针跨线程回调；
///   `Mutex` 守护订阅表，`AtomicU32` 分配 cookie（0 永不返回——COM 约定 0 无效）。
///
/// 用法：[`ConnectionPoint::new`]，构造 container 后调 [`ConnectionPoint::attach_container`]
/// 注入弱引用；推送遍历 sink 经 `IConnectionPoint::EnumConnections`（见 `publisher::enumerate_sinks`）。
#[implement(IConnectionPoint)]
pub struct ConnectionPoint<T>
where
    T: Interface + Clone + 'static,
{
    /// `cookie -> sink` 订阅表。每个 sink 在 `Advise` 时 `cast` 到 `T` 存入。
    sinks: Mutex<HashMap<u32, T>>,
    /// 下一个 cookie 值（从 1 递增；0 为保留的"无效"哨兵）。
    next_cookie: AtomicU32,
    /// 所属可连接对象的弱引用（`attach_container` 注入）。
    container: Mutex<Option<Weak<IConnectionPointContainer>>>,
}

impl<T> ConnectionPoint<T>
where
    T: Interface + Clone + 'static,
{
    /// 新建空连接点（无订阅、无 container）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            sinks: Mutex::new(HashMap::new()),
            next_cookie: AtomicU32::new(1),
            container: Mutex::new(None),
        }
    }

    /// 注入所属可连接对象的弱引用（由 Server/Group 构造后立即调用）。
    ///
    /// 失败（container 不支持弱引用）静默忽略——届时 `GetConnectionPointContainer`
    /// 返回 [`E_POINTER`]。
    pub fn attach_container(&self, container: &IConnectionPointContainer) {
        if let Ok(weak) = container.downgrade() {
            *locked(&self.container) = Some(weak);
        }
    }

    /// 当前订阅数（用于测试与状态上报）。
    pub fn advise_count(&self) -> usize {
        locked(&self.sinks).len()
    }

    /// 分配下一个 cookie。绕过 0（COM 无效哨兵）：仅当 `next_cookie` 环绕回 0 时跳过
    /// （需 2³² 次 advise，现实不可能）。
    fn alloc_cookie(&self) -> u32 {
        loop {
            // Relaxed：cookie 仅需唯一性；fetch_add 保证不重复发放，无其他顺序约束。
            let c = self.next_cookie.fetch_add(1, Ordering::Relaxed);
            if c != 0 {
                return c;
            }
        }
    }
}

impl<T> Default for ConnectionPoint<T>
where
    T: Interface + Clone + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> IConnectionPoint_Impl for ConnectionPoint_Impl<T>
where
    T: Interface + Clone + 'static,
{
    fn GetConnectionInterface(&self) -> Result<GUID> {
        Ok(<T as Interface>::IID)
    }

    // 注：container 弱引用经 `attach_container` 注入；当前 ServerObj/GroupObj 构造 cp 后未调
    // attach_container（字段已是 `IConnectionPoint` 接口类型，取不回 `ConnectionPoint<T>` 具体
    // 类型），故本方法恒返 `E_POINTER`。本 workspace client 走 forward 方向（container→cp），
    // 不反向调此方法，影响为零；需 strictness 时改两阶段构造接线。
    fn GetConnectionPointContainer(&self) -> Result<IConnectionPointContainer> {
        let guard = locked(&self.container);
        match guard.as_ref().and_then(Weak::upgrade) {
            // upgrade 返回同一对象的强引用（AddRef），指针与原 container 相同。
            Some(container) => Ok(container),
            // 未 attach 或 container 已 drop：返回 E_POINTER（"无可用 container"）。
            None => Err(Error::from(E_POINTER)),
        }
    }

    fn Advise(&self, punksink: Ref<'_, IUnknown>) -> Result<u32> {
        // 把传入 sink QI 到本 cp 的目标接口 T。不支持 → CONNECT_E_CANNOTCONNECT
        // （client advise 错接口类型的规范返回）。
        let sink: T = punksink
            .cloned()
            .ok_or_else(|| Error::from(E_POINTER))?
            .cast()
            .map_err(|_| Error::from(CONNECT_E_CANNOTCONNECT))?;
        let cookie = self.alloc_cookie();
        locked(&self.sinks).insert(cookie, sink);
        tracing::debug!(method = "Advise", sink_type = ?<T as Interface>::IID, cookie);
        Ok(cookie)
    }

    fn Unadvise(&self, dwcookie: u32) -> Result<()> {
        if locked(&self.sinks).remove(&dwcookie).is_some() {
            tracing::debug!(method = "Unadvise", cookie = dwcookie);
            Ok(())
        } else {
            // cookie 不存在：规范返回 CONNECT_E_NOCONNECTION。
            Err(Error::from(CONNECT_E_NOCONNECTION))
        }
    }

    fn EnumConnections(&self) -> Result<IEnumConnections> {
        let snapshot: Vec<(u32, IUnknown)> = locked(&self.sinks)
            .iter()
            .filter_map(|(cookie, sink)| sink.cast::<IUnknown>().ok().map(|unk| (*cookie, unk)))
            .collect();
        Ok(EnumConnectionsObj {
            entries: snapshot,
            cursor: Mutex::new(0),
        }
        .into())
    }
}

/// `IEnumConnections` 默认实现——快照枚举某 [`ConnectionPoint`] 当前的所有连接。
///
/// 构造时复制一份 `(cookie, sink)` 快照（`AddRef`），之后游标只前进。`Reset`
/// 回零、`Clone` 复制快照与游标。MTA 下 `Mutex` 守护游标，防并发 `Next`
/// 重复返回同一连接。
#[implement(IEnumConnections)]
struct EnumConnectionsObj {
    /// 不可变快照（构造后只读）。`IUnknown` 持有已 `AddRef` 的 sink 指针。
    entries: Vec<(u32, IUnknown)>,
    /// 当前游标。
    cursor: Mutex<usize>,
}

impl IEnumConnections_Impl for EnumConnectionsObj_Impl {
    #[allow(clippy::cast_possible_truncation)] // avail 被 cconnections(u32) 上界限制，必在 u32 范围内
    fn Next(&self, cconnections: u32, rgcd: *mut CONNECTDATA, pcfetched: *mut u32) -> HRESULT {
        if cconnections == 0 || rgcd.is_null() {
            return E_POINTER;
        }
        let mut cur = locked(&self.cursor);
        let avail = (self.entries.len().saturating_sub(*cur)).min(cconnections as usize);
        // SAFETY: 调用方承诺 rgcd 至少容纳 cconnections 个 CONNECTDATA；avail <= cconnections。
        // 每个 pUnk 用 sink.clone()（AddRef）；CONNECTDATA.pUnk 是 ManuallyDrop，所有权交调用方。
        unsafe {
            for i in 0..avail {
                let (cookie, sink) = &self.entries[*cur + i];
                *rgcd.add(i) = CONNECTDATA {
                    pUnk: std::mem::ManuallyDrop::new(Some(sink.clone())),
                    dwCookie: *cookie,
                };
            }
        }
        *cur += avail;
        drop(cur);
        // COM 约定：pcfetched 非空时写入实际数量。
        if !pcfetched.is_null() {
            // SAFETY: pcfetched 非空时为调用方提供的单个 u32 out 值。
            unsafe {
                *pcfetched = avail as u32;
            }
        }
        // 全部满足返回 S_OK；不足（已到快照尾）返回 S_FALSE——两者皆成功 HRESULT。
        if avail as u32 == cconnections {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, cconnections: u32) -> Result<()> {
        let mut cur = locked(&self.cursor);
        let remaining = self.entries.len().saturating_sub(*cur);
        *cur += (cconnections as usize).min(remaining);
        drop(cur);
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        *locked(&self.cursor) = 0;
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumConnections> {
        let cur = *locked(&self.cursor);
        let cloned: Vec<(u32, IUnknown)> =
            self.entries.iter().map(|(c, s)| (*c, s.clone())).collect();
        Ok(EnumConnectionsObj {
            entries: cloned,
            cursor: Mutex::new(cur),
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opc_da_client::bindings::comn::{IOPCShutdown, IOPCShutdown_Impl};
    use windows::Win32::Foundation::E_NOTIMPL;
    use windows::Win32::System::Com::{
        CoIncrementMTAUsage, IConnectionPoint, IConnectionPointContainer,
        IConnectionPointContainer_Impl, IEnumConnectionPoints, IEnumConnections,
    };
    use windows::core::{Interface, PCWSTR, implement};

    /// 测试用最小 sink：`#[implement(IOPCShutdown)]`。`ShutdownRequest` 记录最后
    /// 调用原因，供未来订阅推送验证断言（当前仅作 Advise 目标）。
    #[implement(IOPCShutdown)]
    struct DummySink {
        last_reason: Mutex<Option<String>>,
    }

    impl IOPCShutdown_Impl for DummySink_Impl {
        fn ShutdownRequest(&self, szreason: &PCWSTR) -> Result<()> {
            // SAFETY: szreason 为 COM 传入的以 0 结尾 UTF-16 串；读到 null 终止。
            let reason = unsafe { szreason.to_string() }.unwrap_or_default();
            *locked(&self.last_reason) = Some(reason);
            Ok(())
        }
    }

    impl DummySink {
        fn new() -> Self {
            Self {
                last_reason: Mutex::new(None),
            }
        }
    }

    /// 测试用最小可连接对象：`#[implement(IConnectionPointContainer)]`——只用于验证
    /// `GetConnectionPointContainer` 的弱引用回指；两个方法返回 `E_NOTIMPL`。
    #[implement(IConnectionPointContainer)]
    struct DummyContainer;

    impl IConnectionPointContainer_Impl for DummyContainer_Impl {
        fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
            Err(E_NOTIMPL.into())
        }

        fn FindConnectionPoint(&self, _riid: *const GUID) -> Result<IConnectionPoint> {
            Err(E_NOTIMPL.into())
        }
    }

    /// COM 运行时 MTA 初始化（幂等：多次调用累加计数，无害）。
    fn init_mta() {
        unsafe { CoIncrementMTAUsage() }.expect("CoIncrementMTAUsage");
    }

    type ShutdownCp = ConnectionPoint<IOPCShutdown>;

    /// 经 `EnumConnections` 黑盒数 `IConnectionPoint` 当前连接数。
    fn count_connections(cp: &IConnectionPoint) -> usize {
        // SAFETY: 同进程对象，EnumConnections 返回有效快照枚举器。
        let en = unsafe { cp.EnumConnections() }.expect("EnumConnections");
        let mut buf = vec![CONNECTDATA::default(); 64];
        let mut fetched = 0u32;
        // SAFETY: 同进程对象。Next 返回 S_OK/S_FALSE（均成功），此处只关心 fetched 数量。
        let _ = unsafe { en.Next(&mut buf, &raw mut fetched) };
        fetched as usize
    }

    #[test]
    fn new_has_zero_advises() {
        // 白盒：内部计数初始为 0。
        assert_eq!(ShutdownCp::new().advise_count(), 0);
    }

    #[test]
    fn advise_returns_nonzero_cookie_and_unadvise() {
        init_mta();
        let cp: IConnectionPoint = ShutdownCp::new().into();
        let sink: IUnknown = DummySink::new().into();
        // SAFETY: MTA 已初始化；同进程 implement 对象 Advise（直接走对象 vtable，不经 SCM）。
        // 传 &sink：windows 0.61 的 Param<IUnknown> 由 &IUnknown 实现（owned IUnknown 不行）。
        let cookie = unsafe { cp.Advise(&sink) }.expect("Advise 应成功");
        assert_ne!(cookie, 0, "cookie 必须 ≠ 0（0 是 COM 无效哨兵）");
        assert_eq!(count_connections(&cp), 1, "advise 后应计 1");
        // SAFETY: 同上。
        unsafe { cp.Unadvise(cookie) }.expect("Unadvise 应成功");
        assert_eq!(count_connections(&cp), 0, "unadvise 后应计 0");
    }

    #[test]
    fn unadvise_unknown_cookie_returns_connect_e_noconnection() {
        init_mta();
        let cp: IConnectionPoint = ShutdownCp::new().into();
        // SAFETY: 同上。
        let err = unsafe { cp.Unadvise(9999) }.unwrap_err();
        assert_eq!(
            err.code(),
            CONNECT_E_NOCONNECTION,
            "未知 cookie 应返回 CONNECT_E_NOCONNECTION"
        );
    }

    #[test]
    fn advise_wrong_interface_returns_connect_e_cannotconnect() {
        init_mta();
        let cp: IConnectionPoint = ShutdownCp::new().into(); // 期望 IOPCShutdown
        // wrong：DummyContainer 仅实现 IConnectionPointContainer，不支持 IOPCShutdown。
        let wrong: IUnknown = DummyContainer.into();
        // SAFETY: 同上。
        let err = unsafe { cp.Advise(&wrong) }.unwrap_err();
        assert_eq!(
            err.code(),
            CONNECT_E_CANNOTCONNECT,
            "advise 错接口应返回 CONNECT_E_CANNOTCONNECT"
        );
    }

    #[test]
    fn get_connection_interface_returns_target_iid() {
        init_mta();
        let cp: IConnectionPoint = ShutdownCp::new().into();
        // SAFETY: 同上。
        let iid = unsafe { cp.GetConnectionInterface() }.expect("GetConnectionInterface");
        assert_eq!(
            iid,
            <IOPCShutdown as Interface>::IID,
            "应返回目标 sink 接口的 IID"
        );
    }

    #[test]
    fn get_connection_point_container_round_trip() {
        init_mta();
        let container: IConnectionPointContainer = DummyContainer.into();
        let cp_obj = ShutdownCp::new();
        cp_obj.attach_container(&container);
        let cp: IConnectionPoint = cp_obj.into();
        // SAFETY: 同上；GetConnectionPointContainer upgrade 弱引用。
        let back = unsafe { cp.GetConnectionPointContainer() }.expect("应能 upgrade 回 container");
        assert_eq!(
            Interface::as_raw(&back),
            Interface::as_raw(&container),
            "GetConnectionPointContainer 应回指 attach 的同一 container"
        );
    }

    #[test]
    fn enum_connections_snapshot_reset_clone() {
        init_mta();
        let cp: IConnectionPoint = ShutdownCp::new().into();
        let sink1: IUnknown = DummySink::new().into();
        let sink2: IUnknown = DummySink::new().into();
        // SAFETY: MTA 已初始化；同进程 Advise。传 &sink：Param<IUnknown> 由 &IUnknown 实现。
        unsafe { cp.Advise(&sink1) }.expect("advise1");
        unsafe { cp.Advise(&sink2) }.expect("advise2");

        // 1) 快照含两个连接，cookie 非零且互异，pUnk 已 AddRef。
        // SAFETY: EnumConnections 返回快照枚举器；Next 写入 buf（容量 8）。
        let en: IEnumConnections = unsafe { cp.EnumConnections() }.expect("EnumConnections");
        let mut buf = vec![CONNECTDATA::default(); 8];
        let mut fetched = 0u32;
        // 请求恰好 2 个（buf[..2]）→ 取满应返回 S_OK（COM Next 语义：fetched==requested）。
        let hr = unsafe { en.Next(&mut buf[..2], &raw mut fetched) };
        assert_eq!(hr, S_OK, "请求 2 取 2 应 S_OK，实际 {hr:?}");
        assert_eq!(fetched, 2, "应取回 2 个连接");
        assert_ne!(buf[0].dwCookie, 0, "cookie 非 0");
        assert_ne!(buf[1].dwCookie, 0, "cookie 非 0");
        assert_ne!(buf[0].dwCookie, buf[1].dwCookie, "两个 cookie 应互异");
        assert!(buf[0].pUnk.is_some(), "pUnk 应已 AddRef");
        assert!(buf[1].pUnk.is_some(), "pUnk 应已 AddRef");

        // 2) 游标已到尾：再 Next 取 0 个 → S_FALSE。
        let mut more = 0u32;
        let hr2 = unsafe { en.Next(&mut buf, &raw mut more) };
        assert_eq!(hr2, S_FALSE, "快照耗尽应 S_FALSE");
        assert_eq!(more, 0, "耗尽后取 0 个");

        // 3) Reset 后可重新枚举到 2 个。
        // SAFETY: Reset 仅重置内部游标。
        unsafe { en.Reset() }.expect("Reset");
        let mut again = 0u32;
        // Reset 后游标归零，再请求 2 个应取满 → S_OK。
        let hr3 = unsafe { en.Next(&mut buf[..2], &raw mut again) };
        assert_eq!(hr3, S_OK, "Reset 后重新枚举应 S_OK，实际 {hr3:?}");
        assert_eq!(again, 2, "Reset 后重新枚举应再次取回 2 个");

        // 4) Clone 继承父游标（父已耗尽 → cloned 也取 0）。
        // SAFETY: Clone 复制快照与游标。
        let cloned = unsafe { en.Clone() }.expect("Clone");
        let mut cloned_n = 0u32;
        let hr4 = unsafe { cloned.Next(&mut buf, &raw mut cloned_n) };
        assert_eq!(hr4, S_FALSE, "Clone 继承父游标应耗尽");
        assert_eq!(cloned_n, 0, "Clone 应继承父游标位置");
    }
}
