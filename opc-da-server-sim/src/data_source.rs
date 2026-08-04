//! `SimDataSource`——实现 `DataSource` trait，按类型表 + count 产生 tag。
//!
//! 镜像 Matrikon.OPC.Simulation 的标签集规模（`count` 个每类型 + 1 单例
//! `_System.Time`）。`read` 按 `TagKind` 分流：`Counter`/`Register` 读原子寄存器，
//! 其余走 `waveform::value` + 按 `dtype` 包 VARIANT。`write` 仅对可写的
//! `Counter`/`Register` 放行（`S_OK`），未知/只读返 `E_ACCESSDENIED`，类型不符
//! 返 `E_INVALIDARG`。

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use opc_da_server::data_source::{
    DataSource, ItemMeta, NamespaceTree, NsOrganization, OPC_QUALITY_BAD, OPC_QUALITY_GOOD,
};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, FILETIME, S_OK, VARIANT_BOOL};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_BOOL, VT_I4, VT_R8};
use windows::core::HRESULT;

use crate::tags::{TYPES, build_namespace_tree, expand_ids};
use crate::waveform::{self, TagKind};

/// 构造 `VT_I4` VARIANT。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn variant_i4(value: i32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 设 vt 判别 + union 字段 lVal；var 按值返回，无别名。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_I4;
        (*var.Anonymous.Anonymous).Anonymous.lVal = value;
    }
    var
}

/// 构造 `VT_R8` VARIANT。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn variant_r8(value: f64) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 同 `variant_i4`；dblVal 为 f64 union 字段。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_R8;
        (*var.Anonymous.Anonymous).Anonymous.dblVal = value;
    }
    var
}

/// 构造 `VT_BOOL` VARIANT。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn variant_bool(value: bool) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 同 `variant_i4`；boolVal 为 VARIANT_BOOL union 字段。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BOOL;
        (*var.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_BOOL::from(value);
    }
    var
}

/// 解析 `VT_I4` VARIANT → `i32`；类型不符返回 `None`。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn variant_as_i4(value: &VARIANT) -> Option<i32> {
    // SAFETY: 只读 vt + lVal；value 由调用方保证有效。
    unsafe {
        if (*value.Anonymous.Anonymous).vt == VT_I4 {
            Some((*value.Anonymous.Anonymous).Anonymous.lVal)
        } else {
            None
        }
    }
}

/// 当前 UTC 时间转 Windows `FILETIME`（1601-01-01 起的 100ns 计数）。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn now_filetime() -> FILETIME {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let intervals = dur
        .as_secs()
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000)
        .saturating_add(u64::from(dur.subsec_nanos()) / 100);
    FILETIME {
        dwLowDateTime: u32::try_from(intervals & 0xFFFF_FFFF).unwrap_or(u32::MAX),
        dwHighDateTime: u32::try_from(intervals >> 32).unwrap_or(u32::MAX),
    }
}

/// 仿真数据源——按 `TYPES` 表 + `count` 展开的 hierarchical tag 集。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时构造；当前仅测试用。
pub struct SimDataSource {
    ns: NamespaceTree,
    start: Instant,
    counter_regs: Vec<AtomicI32>,
    write_regs: Vec<AtomicI32>,
}

impl SimDataSource {
    /// 新建：`count` 个每类型 + 1 单例 `_System.Time`。
    #[must_use]
    #[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时构造；当前仅测试用。
    pub fn new(count: usize) -> Self {
        let ids = expand_ids(count);
        let root = build_namespace_tree(&ids);
        let ns = NamespaceTree::from_tree(root);
        Self {
            ns,
            start: Instant::now(),
            counter_regs: vec_with_atomic(count),
            write_regs: vec_with_atomic(count),
        }
    }
}

/// 解析 item_id → `(type_index, instance_index)`；未知返 `None`。
///
/// 拆为自由函数（不取 `&self`）——纯查 `TYPES` 表，与实例状态无关。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn parse_item(item_id: &str) -> Option<(usize, usize)> {
    for (ti, t) in TYPES.iter().enumerate() {
        if t.singleton && item_id == t.prefix {
            return Some((ti, 0));
        }
        if !t.singleton
            && let Some(rest) = item_id.strip_prefix(t.prefix)
            && let Some(num_str) = rest.strip_prefix('.')
            && let Ok(idx) = num_str.parse::<usize>()
        {
            return Some((ti, idx));
        }
    }
    None
}

/// 构造 `count` 个零初值的 `AtomicI32`（`Vec` 不能直接 `const` 化 atomic，逐个 push）。
#[allow(dead_code)] // Task 5+ main.rs 接入 COM server 时消费；当前仅测试用。
fn vec_with_atomic(count: usize) -> Vec<AtomicI32> {
    (0..count).map(|_| AtomicI32::new(0)).collect()
}

impl DataSource for SimDataSource {
    fn namespace(&self) -> &NamespaceTree {
        &self.ns
    }

    #[allow(clippy::cast_possible_truncation)] // VT_I4 分支：waveform::value 返回 f64 但 Random/ Square/ Saw/ Triangle 均有界 0..=100，截断安全。
    fn read(&self, item_id: &str) -> (VARIANT, u16, FILETIME) {
        let ts = now_filetime();
        let Some((ti, idx)) = parse_item(item_id) else {
            return (VARIANT::default(), OPC_QUALITY_BAD, ts);
        };
        let t = &TYPES[ti];
        let elapsed_secs = self.start.elapsed().as_secs();
        match t.kind {
            TagKind::Counter => {
                // Relaxed：单值，无跨字段顺序约束。
                let v = self
                    .counter_regs
                    .get(idx)
                    .map_or(0, |a| a.load(Ordering::Relaxed));
                (variant_i4(v), OPC_QUALITY_GOOD, ts)
            }
            TagKind::Register => {
                let v = self
                    .write_regs
                    .get(idx)
                    .map_or(0, |a| a.load(Ordering::Relaxed));
                (variant_i4(v), OPC_QUALITY_GOOD, ts)
            }
            _ => {
                let raw = waveform::value(t.kind, idx as u64, elapsed_secs);
                let var = match VARENUM(t.dtype) {
                    VT_I4 => variant_i4(raw as i32),
                    VT_R8 => variant_r8(raw),
                    VT_BOOL => variant_bool(raw != 0.0),
                    _ => VARIANT::default(),
                };
                (var, OPC_QUALITY_GOOD, ts)
            }
        }
    }

    fn write(&self, item_id: &str, value: &VARIANT) -> HRESULT {
        let Some((ti, idx)) = parse_item(item_id) else {
            return E_ACCESSDENIED;
        };
        let t = &TYPES[ti];
        if !t.writable {
            return E_ACCESSDENIED;
        }
        let Some(i) = variant_as_i4(value) else {
            return E_INVALIDARG;
        };
        let regs = match t.kind {
            TagKind::Counter => &self.counter_regs,
            TagKind::Register => &self.write_regs,
            _ => return E_ACCESSDENIED,
        };
        if let Some(slot) = regs.get(idx) {
            slot.store(i, Ordering::Relaxed);
            S_OK
        } else {
            E_INVALIDARG
        }
    }

    fn item_meta(&self, item_id: &str) -> Option<ItemMeta> {
        let (ti, _) = parse_item(item_id)?;
        let t = &TYPES[ti];
        Some(ItemMeta {
            data_type: VARENUM(t.dtype),
            writable: t.writable,
        })
    }

    fn item_range(&self, item_id: &str) -> Option<(f64, f64)> {
        let (ti, _) = parse_item(item_id)?;
        TYPES[ti].range
    }

    fn query_organization(&self) -> NsOrganization {
        NsOrganization::Hierarchical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_size_and_org() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.namespace().leaves().len(), 8 * 5 + 1, "41 tag");
        assert_eq!(
            ds.query_organization(),
            opc_da_server::data_source::NsOrganization::Hierarchical
        );
    }

    #[test]
    fn read_random_int4_is_good_vt_i4() {
        let ds = SimDataSource::new(5);
        let (v, q, _ts) = ds.read("Random.Int4.0");
        assert_eq!(q, OPC_QUALITY_GOOD);
        assert_eq!(vt_of(&v), VT_I4);
    }

    #[test]
    fn read_altbool_is_vt_bool() {
        let ds = SimDataSource::new(5);
        let (v, q, _) = ds.read("AltBool.Bool.0");
        assert_eq!(q, OPC_QUALITY_GOOD);
        assert_eq!(vt_of(&v), VT_BOOL);
    }

    #[test]
    fn read_system_time_singleton() {
        let ds = SimDataSource::new(5);
        let (v, q, _) = ds.read("_System.Time");
        assert_eq!(q, OPC_QUALITY_GOOD);
        assert_eq!(vt_of(&v), VT_R8);
    }

    #[test]
    fn read_unknown_is_bad() {
        let ds = SimDataSource::new(5);
        let (_v, q, _) = ds.read("nope.0");
        assert_eq!(q, OPC_QUALITY_BAD);
    }

    #[test]
    fn write_counter_round_trip() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.write("BucketBrigade.Int4.2", &variant_i4(42)), S_OK);
        let (v, q, _) = ds.read("BucketBrigade.Int4.2");
        assert_eq!(q, OPC_QUALITY_GOOD);
        assert_eq!(variant_as_i4(&v), Some(42));
    }

    #[test]
    fn write_register_round_trip() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.write("WriteTag.Int4.1", &variant_i4(7)), S_OK);
        let (v, _, _) = ds.read("WriteTag.Int4.1");
        assert_eq!(variant_as_i4(&v), Some(7));
    }

    #[test]
    fn write_readonly_denied() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.write("Random.Int4.0", &variant_i4(1)), E_ACCESSDENIED);
    }

    #[test]
    fn write_wrong_type_invalidarg() {
        let ds = SimDataSource::new(5);
        assert_eq!(
            ds.write("BucketBrigade.Int4.0", &variant_r8(1.0)),
            E_INVALIDARG
        );
    }

    #[test]
    fn write_unknown_denied() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.write("nope.0", &variant_i4(1)), E_ACCESSDENIED);
    }

    #[test]
    fn item_meta_known_and_writable_flag() {
        let ds = SimDataSource::new(5);
        let m = ds.item_meta("BucketBrigade.Int4.0").expect("meta");
        assert_eq!(m.data_type, VT_I4);
        assert!(m.writable);
        let ro = ds.item_meta("Random.Real8.0").expect("meta");
        assert!(!ro.writable);
        assert!(ds.item_meta("nope").is_none());
    }

    #[test]
    fn item_range_eu() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.item_range("Random.Int4.0"), Some((0.0, 100.0)));
        assert_eq!(ds.item_range("WriteTag.Int4.0"), None);
    }

    /// 读 VARIANT 的 vt 判别（测试辅助，集中 unsafe）。
    fn vt_of(v: &VARIANT) -> VARENUM {
        // SAFETY: 只读 vt 判别字段；v 是自构造有效 VARIANT。
        unsafe { (*v.Anonymous.Anonymous).vt }
    }
}
