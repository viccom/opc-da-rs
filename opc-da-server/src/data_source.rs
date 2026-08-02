//! 数据源抽象 + 默认 `SimDataSource`——阶段 1。
//!
//! server 的所有数据来自 [`DataSource`]（设计 §9）。`SimDataSource` 镜像
//! Matrikon.OPC.Simulation 的标签集（`Random.Int4` / `Random.Real8` /
//! `Square Waves.Real8` / `Bucket Brigade.Int4`），值产生器在 `read` 时按
//! 经过时间计算（read-time），可写 tag（Bucket Brigade）用 [`AtomicI32`] 持久化。
//!
//! 设计 §9 描述的"后台 tokio task 周期刷新缓存"在此用 **read-time 计算**实现：
//! 随机/方波/计数器的值都是时间的函数，`read` 取当前时刻值等价于读取一个被周期
//! 刷新的缓存。`IOPCSyncIO::Read` 完全正确；publisher 引擎（§10）若需推送缓存
//! 再加独立 task。这是未来"协议网关 DataSource"（Modbus/S7/UA 桥接）的扩展点。

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, FILETIME, S_OK};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_I4, VT_R8};
use windows::core::HRESULT;

/// OPC 质量掩码（低 6 位为质量；高 2 位为 limit）。`GOOD`=0xC0，`BAD`=0x00。
/// 与 `opc-da-client` 的 quality 解析一致（Matrikon / OPC DA 规范）。
pub const OPC_QUALITY_GOOD: u16 = 0xC0;
pub const OPC_QUALITY_BAD: u16 = 0x00;

/// 命名空间——server 暴露的可寻址 item 列表（browse 用）。
///
/// `SimDataSource` 用 flat 命名空间（点号分隔的 leaf id，如 `Random.Int4`）。
/// hierarchical 分支结构留待阶段 2（`IOPCBrowseServerAddressSpace`）。
#[derive(Debug, Clone)]
pub struct NamespaceTree {
    leaves: Vec<String>,
}

impl NamespaceTree {
    /// 从 leaf id 列表构造。
    #[must_use]
    pub fn new(leaves: Vec<String>) -> Self {
        Self { leaves }
    }

    /// 所有 leaf id（browse 顺序）。
    #[must_use]
    pub fn leaves(&self) -> &[String] {
        &self.leaves
    }
}

/// 单个 item 的规范元数据（`AddItems` 用：校验 data_type / access_rights）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemMeta {
    /// 规范 COM VARIANT 类型（`VT_I4` / `VT_R8` / …）。
    pub data_type: VARENUM,
    /// 是否可写（只读 tag 的 `Write` 返回 `E_ACCESSDENIED`）。
    pub writable: bool,
}

/// server 的"虚拟工厂"——所有数据的来源（设计 §9）。
///
/// 实现者：[`SimDataSource`]（内置）；未来可加协议网关实现（Modbus/S7/UA 桥接）。
/// 所有方法在 COM 调用线程（worker）同步执行——实现者负责线程安全（`Send + Sync`）。
pub trait DataSource: Send + Sync {
    /// 命名空间（browse 用）。
    fn namespace(&self) -> &NamespaceTree;
    /// 同步读一个 item：返回 `(value, quality, timestamp)`。未知 item 返回
    /// 空 VARIANT + [`OPC_QUALITY_BAD`]。
    fn read(&self, item_id: &str) -> (VARIANT, u16, FILETIME);
    /// 写一个 item。返回 COM HRESULT（`S_OK` 成功 / `E_ACCESSDENIED` 只读 /
    /// `E_INVALIDARG` 类型不符）。
    fn write(&self, item_id: &str, value: &VARIANT) -> HRESULT;
    /// 该 item 的规范元数据；未知 item 返回 `None`（`AddItems` 据此拒收）。
    fn item_meta(&self, item_id: &str) -> Option<ItemMeta>;
}

/// 内置仿真数据源——镜像 Matrikon.OPC.Simulation 标签集。
///
/// | item_id | 类型 | 行为 |
/// |---|---|---|
/// | `Random.Int4` | `VT_I4` | 0..=100 伪随机，每秒变（read-time） |
/// | `Random.Real8` | `VT_R8` | 0.0..100.0 伪随机，每秒变（read-time） |
/// | `Square Waves.Real8` | `VT_R8` | 100.0/0.0 方波，每秒切换 |
/// | `Bucket Brigade.Int4` | `VT_I4` | 可写计数器（`Write` 设值，`Read` 返回最近值） |
pub struct SimDataSource {
    ns: NamespaceTree,
    start: Instant,
    /// `Bucket Brigade.Int4` 的当前值（write 设，read 读）。原子操作无锁，无 poison。
    bucket: AtomicI32,
}

impl SimDataSource {
    /// 新建仿真数据源（4 内置 tag，bucket 初值 0，时间基准 = now）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            ns: NamespaceTree::new(vec![
                "Random.Int4".into(),
                "Random.Real8".into(),
                "Square Waves.Real8".into(),
                "Bucket Brigade.Int4".into(),
            ]),
            start: Instant::now(),
            bucket: AtomicI32::new(0),
        }
    }
}

impl Default for SimDataSource {
    fn default() -> Self {
        Self::new()
    }
}

/// 内置 tag 的元数据表。返回 `(data_type, writable)`；未知返回 `None`。
fn tag_meta(item_id: &str) -> Option<(VARENUM, bool)> {
    match item_id {
        "Random.Int4" => Some((VT_I4, false)),
        "Random.Real8" | "Square Waves.Real8" => Some((VT_R8, false)),
        "Bucket Brigade.Int4" => Some((VT_I4, true)),
        _ => None,
    }
}

impl DataSource for SimDataSource {
    fn namespace(&self) -> &NamespaceTree {
        &self.ns
    }

    fn read(&self, item_id: &str) -> (VARIANT, u16, FILETIME) {
        let elapsed = self.start.elapsed();
        let ts = now_filetime();
        match item_id {
            "Random.Int4" => (variant_i4(random_i4(elapsed)), OPC_QUALITY_GOOD, ts),
            "Random.Real8" => (variant_r8(random_r8(elapsed)), OPC_QUALITY_GOOD, ts),
            "Square Waves.Real8" => (variant_r8(square_wave(elapsed)), OPC_QUALITY_GOOD, ts),
            "Bucket Brigade.Int4" => {
                // Relaxed：bucket 单值，无跨字段顺序约束。
                let v = self.bucket.load(Ordering::Relaxed);
                (variant_i4(v), OPC_QUALITY_GOOD, ts)
            }
            _ => (VARIANT::default(), OPC_QUALITY_BAD, ts),
        }
    }

    fn write(&self, item_id: &str, value: &VARIANT) -> HRESULT {
        match item_id {
            "Bucket Brigade.Int4" => match variant_as_i4(value) {
                Some(i) => {
                    self.bucket.store(i, Ordering::Relaxed);
                    S_OK
                }
                None => E_INVALIDARG, // 非 VT_I4
            },
            // 其他 tag 只读。
            _ => E_ACCESSDENIED,
        }
    }

    fn item_meta(&self, item_id: &str) -> Option<ItemMeta> {
        tag_meta(item_id).map(|(data_type, writable)| ItemMeta {
            data_type,
            writable,
        })
    }
}

// —— VARIANT 构造/解析辅助 ——
// 参考 `opc-da-client/src/helpers.rs::opc_value_to_variant` 的 VARIANT union 访问
// 模式（client 的该函数 pub 但 `helpers` 模块私有；按 RUNBOOK §0"为 server 改
// client 禁止"，server 侧自实现，不暴露 client internals）。

/// 构造 `VT_I4` VARIANT。
fn variant_i4(value: i32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 设 vt 判别 + 对应 union 字段 lVal。var 按值返回，无并发/别名。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_I4;
        (*var.Anonymous.Anonymous).Anonymous.lVal = value;
    }
    var
}

/// 构造 `VT_R8` VARIANT。
fn variant_r8(value: f64) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 同 `variant_i4`；dblVal 为 f64 union 字段。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_R8;
        (*var.Anonymous.Anonymous).Anonymous.dblVal = value;
    }
    var
}

/// 解析 `VT_I4` VARIANT → `i32`；类型不符返回 `None`。
fn variant_as_i4(value: &VARIANT) -> Option<i32> {
    // SAFETY: 只读 vt 判别 + lVal 字段；value 由调用方保证有效（COM 传入或自构造）。
    unsafe {
        if (*value.Anonymous.Anonymous).vt == VT_I4 {
            Some((*value.Anonymous.Anonymous).Anonymous.lVal)
        } else {
            None
        }
    }
}

// —— read-time 值产生器 —— 确定性，基于经过时间（镜像 Matrikon Simulation 行为）——

/// `Random.Int4`：0..=100 伪随机 i32，每秒变（Knuth 乘法 hash + 模 101）。
fn random_i4(elapsed: Duration) -> i32 {
    let seed = elapsed.as_secs();
    let mixed = seed.wrapping_mul(2_654_435_761) % 101;
    i32::try_from(mixed).unwrap_or(0)
}

/// `Random.Real8`：0.0..100.0 伪随机 f64，每秒变。
fn random_r8(elapsed: Duration) -> f64 {
    let seed = elapsed.as_secs();
    let mixed = seed.wrapping_mul(2_654_435_761) % 10_000;
    f64::from(u32::try_from(mixed).unwrap_or(0)) / 100.0
}

/// `Square Waves.Real8`：每秒在 100.0 / 0.0 间切换的方波。
fn square_wave(elapsed: Duration) -> f64 {
    if elapsed.as_secs().is_multiple_of(2) {
        100.0
    } else {
        0.0
    }
}

/// 当前 UTC 时间转 Windows `FILETIME`（1601-01-01 起的 100ns 计数）。
///
/// 用 `std::time::SystemTime` 手工转换，避免引入 `Win32_System_Time` feature。
fn now_filetime() -> FILETIME {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // 1601-01-01 与 1970-01-01 相差 11644473600 秒；FILETIME 单位 = 100ns。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 读 VARIANT 的 vt 判别（测试辅助，集中 unsafe）。
    fn vt_of(v: &VARIANT) -> VARENUM {
        // SAFETY: 只读 vt 字段；v 是自构造有效 VARIANT。
        unsafe { (*v.Anonymous.Anonymous).vt }
    }

    #[test]
    fn namespace_lists_sim_tags() {
        let ds = SimDataSource::new();
        let leaves = ds.namespace().leaves();
        assert_eq!(leaves.len(), 4, "4 内置 tag");
        assert!(leaves.contains(&"Random.Int4".to_string()));
        assert!(leaves.contains(&"Bucket Brigade.Int4".to_string()));
    }

    #[test]
    fn item_meta_known_and_unknown() {
        let ds = SimDataSource::new();
        let random = ds.item_meta("Random.Int4").expect("Random.Int4 meta");
        assert_eq!(random.data_type, VT_I4);
        assert!(!random.writable, "Random 应只读");

        let bucket = ds
            .item_meta("Bucket Brigade.Int4")
            .expect("Bucket Brigade meta");
        assert_eq!(bucket.data_type, VT_I4);
        assert!(bucket.writable, "Bucket Brigade 应可写");

        assert!(ds.item_meta("nope").is_none(), "未知 item 无 meta");
    }

    /// 读核心意图：已知 tag 返回正确类型 + GOOD quality。
    #[test]
    fn read_known_tag_is_good_quality_with_right_type() {
        let ds = SimDataSource::new();
        let (v, q, _ts) = ds.read("Random.Int4");
        assert_eq!(q, OPC_QUALITY_GOOD, "Random.Int4 应 GOOD");
        assert_eq!(vt_of(&v), VT_I4, "Random.Int4 应 VT_I4");

        let (vr, qr, _) = ds.read("Random.Real8");
        assert_eq!(qr, OPC_QUALITY_GOOD);
        assert_eq!(vt_of(&vr), VT_R8, "Random.Real8 应 VT_R8");
    }

    /// 读核心意图：未知 item 不 panic，返回 BAD quality（client 据此判失败）。
    #[test]
    fn read_unknown_tag_is_bad_quality() {
        let ds = SimDataSource::new();
        let (_v, q, _ts) = ds.read("nonexistent.tag");
        assert_eq!(q, OPC_QUALITY_BAD, "未知 item 应 BAD");
    }

    /// 写核心意图：可写 tag 持久化，read 反映写入值（round trip）。
    #[test]
    fn write_bucket_brigade_persists_then_read_reflects() {
        let ds = SimDataSource::new();
        let v = variant_i4(42);
        assert_eq!(
            ds.write("Bucket Brigade.Int4", &v),
            S_OK,
            "写 Bucket Brigade 应成功"
        );
        let (read_v, q, _) = ds.read("Bucket Brigade.Int4");
        assert_eq!(q, OPC_QUALITY_GOOD);
        assert_eq!(variant_as_i4(&read_v), Some(42), "read 应反映写入的 42");
    }

    /// 写核心意图：只读 tag 拒绝（E_ACCESSDENIED）。
    #[test]
    fn write_readonly_tag_denied() {
        let ds = SimDataSource::new();
        let v = variant_i4(1);
        assert_eq!(
            ds.write("Random.Int4", &v),
            E_ACCESSDENIED,
            "Random 只读，写应拒绝"
        );
    }

    /// 写核心意图：类型不符（非 VT_I4）拒绝（E_INVALIDARG）。
    #[test]
    fn write_wrong_type_rejected() {
        let ds = SimDataSource::new();
        let v = variant_r8(1.0); // Bucket Brigade 期望 VT_I4
        assert_eq!(
            ds.write("Bucket Brigade.Int4", &v),
            E_INVALIDARG,
            "非 VT_I4 应 E_INVALIDARG"
        );
    }

    /// read-time 值随时间变化（Random 每秒变）。
    #[test]
    fn random_changes_over_time() {
        let ds = SimDataSource::new();
        let (v0, _, _) = ds.read("Random.Int4");
        let i0 = variant_as_i4(&v0).unwrap_or(-1);
        // 同一秒内可能相同；这里只验证值在 0..=100 范围内（read-time 产生器约束）。
        assert!(
            (0..=100).contains(&i0),
            "Random.Int4 应在 0..=100，实际 {i0}"
        );
    }
}
