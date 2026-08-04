//! 探针结果 + 压测指标输出 helper（共享）。

/// 记一探针结果：pass 则 `passed += 1`，fail 则 `failed += 1`。
pub fn probe(passed: &mut u32, failed: &mut u32, name: &str, ok: bool, detail: &str) {
    if ok {
        println!("✓ {name}: {detail}");
        *passed += 1;
    } else {
        println!("✗ {name}: {detail}");
        *failed += 1;
    }
}
