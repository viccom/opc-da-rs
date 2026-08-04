//! 值生成器——read-time 纯计算（无 IO，确定性）。
//!
//! 仅覆盖"时间函数"类波形（random/square/sawtooth/triangle/altbool/systime）。
//! `Counter`/`Register` 读寄存器，在 `data_source.rs` 的 `read` 里直接处理，不走本模块。

/// tag 值的生成方式（与 `tags::TagType.wf` 对应）。
///
/// `SysTime`/`Counter`/`Register` 在本模块内仅 `match` 不构造——它们由后续 task 产出：
/// `tags::TagType`（Task 3）持有 `wf: TagKind`，`Counter`/`Register` 在 `data_source.rs`
/// （Task 4）的 `read` 分流时构造。在此之前 dead-code 检查会误报，故显式 allow。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    Random,
    Square,
    Sawtooth,
    Triangle,
    AltBool,
    SysTime,
    Counter,
    Register,
}

/// 按 `kind` 生成 read-time 值（f64）。`Counter`/`Register` 不应调用本函数（caller 分流）。
///
/// - `index` = tag 实例序号（用于相位错开）
/// - `elapsed_secs` = server 启动至今的整秒
///
/// 当前仅测试调用——Task 3（`tags`）/Task 4（`data_source`）落地后由它们构造并消费。
#[must_use]
#[allow(dead_code)]
pub fn value(kind: TagKind, index: u64, elapsed_secs: u64) -> f64 {
    let t = elapsed_secs.wrapping_add(index);
    match kind {
        TagKind::Random => {
            let mixed = t.wrapping_mul(2_654_435_761) % 101;
            f64::from(u32::try_from(mixed).unwrap_or(0))
        }
        TagKind::Square => {
            if t.is_multiple_of(2) {
                100.0
            } else {
                0.0
            }
        }
        TagKind::Sawtooth => {
            let p = t % 10;
            f64::from(u32::try_from(p).unwrap_or(0)) / 10.0 * 100.0
        }
        TagKind::Triangle => {
            let p = t % 10;
            let tri = if p < 5 { p } else { 10 - p };
            f64::from(u32::try_from(tri).unwrap_or(0)) / 5.0 * 100.0
        }
        TagKind::AltBool => {
            if t.is_multiple_of(2) {
                1.0
            } else {
                0.0
            }
        }
        TagKind::SysTime => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64()),
        TagKind::Counter | TagKind::Register => 0.0,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // 浮点比较均为 bit 级确定性（square/altbool/sawtooth/triangle 全是整数 × 幂运算结果，无误差累积）
mod tests {
    use super::*;

    #[test]
    fn random_in_range_and_deterministic() {
        let a = value(TagKind::Random, 3, 100);
        let b = value(TagKind::Random, 3, 100);
        assert_eq!(a, b, "同种子必确定");
        assert!(value(TagKind::Random, 0, 0) < 100.0);
        assert!(value(TagKind::Random, 0, 0) >= 0.0);
    }

    #[test]
    fn square_is_0_or_100() {
        assert_eq!(value(TagKind::Square, 0, 0), 100.0);
        assert_eq!(value(TagKind::Square, 1, 0), 0.0);
        assert_eq!(value(TagKind::Square, 0, 1), 0.0);
    }

    #[test]
    fn sawtooth_in_range() {
        for s in 0..20 {
            let v = value(TagKind::Sawtooth, 0, s);
            assert!((0.0..100.0).contains(&v), "sawtooth {v} 越界 (s={s})");
        }
    }

    #[test]
    fn triangle_in_range_and_peaks() {
        assert_eq!(value(TagKind::Triangle, 0, 0), 0.0);
        let mid = value(TagKind::Triangle, 0, 5);
        assert!((mid - 100.0).abs() < 1e-9, "三角峰值应 100，实际 {mid}");
    }

    #[test]
    fn alt_bool_toggles() {
        assert_eq!(value(TagKind::AltBool, 0, 0), 1.0);
        assert_eq!(value(TagKind::AltBool, 0, 1), 0.0);
    }

    #[test]
    fn index_shifts_phase() {
        let differ = (0..10).any(|i| {
            let a = value(TagKind::Sawtooth, i, 7);
            let b = value(TagKind::Sawtooth, 0, 7);
            a != b
        });
        assert!(differ, "index 应错开相位");
    }
}
