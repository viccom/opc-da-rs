# opc-da-server-sim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 创建 `opc-da-server-sim`——基于 `opc-da-server` 库的薄包装示例 OPC DA Simulation Server（对标 `Matrikon.OPC.Simulation.1`），作为 workspace 第 6 个 member。

**Architecture:** 纯 bin crate，复制 `opc-da-server/src/bin/opc-da-server.rs` 的 COM 编排骨架，替换 ProgID/CLSID + 自定义 `SimDataSource`（8 类型模板 × count + `_System.Time` 单例，按 `.` 建 hierarchical 树）。tag 数量由 env `OPC_DA_SIM_COUNT`（默认 100）控制。

**Tech Stack:** Rust 2024 / windows-rs 0.61 COM / `opc-da-server`（path）/ `opc-da-client`（CATID 常量）。Windows-only。

**上游 spec:** `docs/superpowers/specs/2026-08-04-opc-da-server-sim-design.md`

**关键库 API（已核对源码）:**
- `opc_da_server::class_factory::{Factory, CLSID_OPC_DA_SERVER}` — `Factory::new(Arc<dyn DataSource>) -> Self`，`.into() -> IClassFactory`
- `opc_da_server::objects::scheduler::init(usize)`
- `opc_da_server::registry::{ServerRegistration, register, unregister}`
- `opc_da_server::data_source::{DataSource, NamespaceTree, NsNode, NsOrganization, ItemMeta, OPC_QUALITY_GOOD, OPC_QUALITY_BAD}`
- `opc_da_client::bindings::da::{CATID_OPCDAServer10, CATID_OPCDAServer20, CATID_OPCDAServer30}`
- 库的 VARIANT helper（`variant_i4`/`variant_r8`/`variant_as_i4`/`now_filetime`）是 `pub(crate)`——sim 必须自实现（复制）

---

## File Structure

```
opc-da-server-sim/
├── Cargo.toml           # 新建。name=opc-da-server-sim；deps: opc-da-server, opc-da-client, windows
├── README.md            # 新建。仿 opc-da-desktop/README.md
└── src/
    ├── main.rs          # 新建。mod 声明 + args 解析 + main()
    ├── waveform.rs      # 新建。enum TagKind + value() 纯函数 + 单测
    ├── tags.rs          # 新建。TagType + TYPES 表 + expand_ids() + build_namespace_tree() + 单测
    ├── data_source.rs   # 新建。SimDataSource + VARIANT helpers + read/write + 单测
    └── runtime.rs       # 新建。CLSID/ProgID 常量 + build_registration() + run() COM 编排
```

修改的外部文件：
- `Cargo.toml`（workspace 根）— `members` 加 `"opc-da-server-sim"`
- `opc-da-client-test/src/server_proc.rs` — 加 `OPC_DA_SERVER_PROGID` env 透传
- `opc-da-client-test/src/e2e.rs` + `stress.rs` — `PROG_ID` 改为读 env

依赖顺序：`waveform`（无依赖）→ `tags`（依赖 waveform）→ `data_source`（依赖 tags+waveform）→ `runtime`（依赖 data_source）→ `main`（依赖 runtime）。

---

## Task 1: crate 骨架 + workspace 注册

**Files:**
- Create: `opc-da-server-sim/Cargo.toml`
- Create: `opc-da-server-sim/src/main.rs`
- Modify: `Cargo.toml`（workspace 根，`members` 数组）

- [ ] **Step 1: 创建 `opc-da-server-sim/Cargo.toml`**

```toml
[package]
name = "opc-da-server-sim"
authors.workspace = true
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
description = "OPC DA Simulation Server example built on opc-da-server (mirrors Matrikon.OPC.Simulation.1)"
license.workspace = true
repository.workspace = true
readme = "README.md"
keywords = ["opc", "opc-da", "simulation", "windows", "scada"]
categories = ["api-bindings", "os::windows-apis"]

[package.metadata.docs.rs]
default-target = "x86_64-pc-windows-msvc"
targets = ["x86_64-pc-windows-msvc"]

[lints]
workspace = true

[dependencies]
opc-da-server = { path = "../opc-da-server" }
opc-da-client = { path = "../opc-da-client" }

[target.'cfg(windows)'.dependencies]
windows = { workspace = true }
```

> sim 不引入 anyhow/tokio/tracing/clap。错误用 `eprintln!` + `process::exit(1)`（与库 bin 风格一致）。

- [ ] **Step 2: 创建 `opc-da-server-sim/src/main.rs`（占位，后续 task 填充）**

```rust
//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! Windows-only（依赖 opc-da-server 的 COM 实现）。非 Windows 由 opc-da-server 的
//! `compile_error!` 直接拒编译（与库一致，无需额外 stub）。

fn main() {
    eprintln!("opc-da-server-sim: skeleton (Task 1) — 后续 task 填充 COM 编排");
}
```

- [ ] **Step 3: workspace 根 `Cargo.toml` 加 member**

把 `members` 数组改为（在 `"opc-da-server"` 后加一行）：

```toml
members = [
    "opc-cli",
    "opc-da-client",
    "opc-da-client-test",
    "opc-da-desktop",
    "opc-da-server",
    "opc-da-server-sim",
]
```

- [ ] **Step 4: 验证编译**

Run: `cargo build -p opc-da-server-sim`
Expected: 编译成功（Windows）。

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml opc-da-server-sim/Cargo.toml opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): crate 骨架 + workspace 注册"
```

---

## Task 2: waveform.rs（TagKind + 纯函数值生成器）

值生成逻辑是纯 f64 计算（不碰 VARIANT/unsafe），最先做、最易测。`Counter`/`Register` 不在此（它们读寄存器，在 Task 4 处理）。

**Files:**
- Create: `opc-da-server-sim/src/waveform.rs`
- Modify: `opc-da-server-sim/src/main.rs`（加 `mod waveform;`）

- [ ] **Step 1: 写失败测试（先加 `mod waveform;` 再写测试）**

修改 `src/main.rs`，在 `fn main()` 上方加：

```rust
#[cfg(windows)]
mod waveform;
```

创建 `src/waveform.rs`，先只写测试（实现留空触发失败）：

```rust
//! 值生成器——read-time 纯计算（无 IO，确定性）。
//!
//! 仅覆盖"时间函数"类波形（random/square/sawtooth/triangle/altbool/systime）。
//! `Counter`/`Register` 读寄存器，在 `data_source.rs` 的 `read` 里直接处理，不走本模块。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_in_range_and_deterministic() {
        // 同 (index, secs) 必出同值；值 ∈ [0, 100)。
        let a = value(TagKind::Random, 3, 100);
        let b = value(TagKind::Random, 3, 100);
        assert_eq!(a, b, "同种子必确定");
        assert!(value(TagKind::Random, 0, 0) < 100.0);
        assert!(value(TagKind::Random, 0, 0) >= 0.0);
    }

    #[test]
    fn square_is_0_or_100() {
        assert_eq!(value(TagKind::Square, 0, 0), 100.0); // (0+0)%2==0 → 100
        assert_eq!(value(TagKind::Square, 1, 0), 0.0);   // (0+1)%2==1 → 0
        assert_eq!(value(TagKind::Square, 0, 1), 0.0);   // (1+0)%2==1 → 0
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
        // 周期 10：index 0 时 p=0→0；某点应接近 100。
        assert_eq!(value(TagKind::Triangle, 0, 0), 0.0);
        let mid = value(TagKind::Triangle, 0, 5); // p=5 → t=5 → 100
        assert!((mid - 100.0).abs() < 1e-9, "三角峰值应 100，实际 {mid}");
    }

    #[test]
    fn alt_bool_toggles() {
        assert_eq!(value(TagKind::AltBool, 0, 0), 1.0); // even → 1
        assert_eq!(value(TagKind::AltBool, 0, 1), 0.0); // odd → 0
    }

    #[test]
    fn index_shifts_phase() {
        // 不同 index 在同一时刻至少存在一组不同值（相位错开）。
        let differ = (0..10).any(|i| {
            let a = value(TagKind::Sawtooth, i, 7);
            let b = value(TagKind::Sawtooth, 0, 7);
            a != b
        });
        assert!(differ, "index 应错开相位");
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p opc-da-server-sim waveform`
Expected: 编译失败（`TagKind` / `value` 未定义）。

- [ ] **Step 3: 写实现**

在 `src/waveform.rs` 顶部（测试 mod 之前）加：

```rust
//! 值生成器——read-time 纯计算（无 IO，确定性）。
//!
//! 仅覆盖"时间函数"类波形（random/square/sawtooth/triangle/altbool/systime）。
//! `Counter`/`Register` 读寄存器，在 `data_source.rs` 的 `read` 里直接处理，不走本模块。

/// tag 值的生成方式（与 `tags::TagType.wf` 对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagKind {
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
#[must_use]
pub(crate) fn value(kind: TagKind, index: u64, elapsed_secs: u64) -> f64 {
    let t = elapsed_secs.wrapping_add(index);
    match kind {
        TagKind::Random => {
            // Knuth 乘法 hash + 模 101（确定性，每秒变）。
            let mixed = t.wrapping_mul(2_654_435_761) % 101;
            f64::from(u32::try_from(mixed).unwrap_or(0))
        }
        TagKind::Square => {
            if t.is_multiple_of(2) { 100.0 } else { 0.0 }
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
            if t.is_multiple_of(2) { 1.0 } else { 0.0 }
        }
        TagKind::SysTime => {
            // UNIX epoch 秒（caller 包装为 VT_R8）。
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        }
        // Counter/Register 由 data_source::read 直接读寄存器，不走本函数。
        TagKind::Counter | TagKind::Register => 0.0,
    }
}
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p opc-da-server-sim waveform`
Expected: 6 个测试全 PASS。

- [ ] **Step 5: clippy 单模块**

Run: `cargo clippy -p opc-da-server-sim --all-targets -- -D warnings`
Expected: 无 warning（`is_multiple_of` 需 Rust ≥ 1.87，本项目 1.93.1 OK）。

- [ ] **Step 6: Commit**

```bash
git add opc-da-server-sim/src/waveform.rs opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): waveform 值生成器 + 单测"
```

---

## Task 3: tags.rs（类型表 + count 展开 + 建树）

**Files:**
- Create: `opc-da-server-sim/src/tags.rs`
- Modify: `opc-da-server-sim/src/main.rs`（加 `mod tags;`）

- [ ] **Step 1: main.rs 加 mod 声明**

```rust
#[cfg(windows)]
mod waveform;
#[cfg(windows)]
mod tags;
```

- [ ] **Step 2: 写失败测试**

创建 `src/tags.rs`，先写测试：

```rust
//! tag 类型表 + count 展开 + 按 '.' 建 hierarchical 命名空间树。

use std::sync::Arc;

use opc_da_server::data_source::NsNode;

use crate::waveform::TagKind;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_default_count() {
        // 8 展开类型 × 100 + _System.Time 单例 = 801。
        let ids = expand_ids(100);
        assert_eq!(ids.len(), 8 * 100 + 1, "默认 801 tag");
        assert!(ids.contains(&"_System.Time".to_string()), "含单例");
        assert!(ids.contains(&"Random.Int4.0".to_string()));
        assert!(ids.contains(&"Random.Int4.99".to_string()));
        assert!(!ids.contains(&"Random.Int4.100".to_string()), "index 上限 99");
    }

    #[test]
    fn expand_small_count() {
        let ids = expand_ids(2);
        assert_eq!(ids.len(), 8 * 2 + 1, "17 tag");
        assert!(ids.contains(&"BucketBrigade.Int4.1".to_string()));
    }

    #[test]
    fn expand_no_duplicates() {
        let ids = expand_ids(50);
        let mut sorted = ids.clone();
        sorted.sort();
        let dedup_len = sorted.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(dedup_len, 0, "无重复 item_id");
    }

    #[test]
    fn tree_random_branch() {
        let ids = expand_ids(3);
        let root = build_namespace_tree(&ids);
        let children = match &root {
            NsNode::Branch { children, .. } => children,
            NsNode::Leaf { .. } => panic!("root 必为 Branch"),
        };
        // root 下应有 Random / Square / Sawtooth / Triangle / BucketBrigade / WriteTag / AltBool / _System 分支。
        let names: Vec<&str> = children.iter().filter_map(|c| match c {
            NsNode::Branch { name, .. } => Some(name.as_ref()),
            NsNode::Leaf { .. } => None,
        }).collect();
        assert!(names.contains(&"Random"), "缺 Random 分支");
        assert!(names.contains(&"_System"), "缺 _System 分支");
    }

    #[test]
    fn tree_random_int4_has_3_leaves() {
        let ids = expand_ids(3);
        let root = build_namespace_tree(&ids);
        // browse Random → Int4 → 3 个数字叶。
        let n = opc_da_server::data_source::NamespaceTree::from_tree(root);
        assert_eq!(n.browse_children(&["Random"]).len(), 2, "Random 下 Int4/Real8");
        assert_eq!(n.browse_children(&["Random", "Int4"]).len(), 3, "3 个 index 叶");
    }
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p opc-da-server-sim tags`
Expected: 编译失败（`TagType`/`TYPES`/`expand_ids`/`build_namespace_tree` 未定义）。

- [ ] **Step 4: 写实现**

在 `src/tags.rs` 测试 mod 之前加：

```rust
//! tag 类型表 + count 展开 + 按 '.' 建 hierarchical 命名空间树。

use std::sync::Arc;

use opc_da_server::data_source::NsNode;

use crate::waveform::TagKind;

/// 单个 tag 类型的定义。
pub(crate) struct TagType {
    pub prefix: &'static str,
    pub dtype: u16, // VARENUM（VT_I4 等），用 u16 避免在此模块 import 全部 VT 常量
    pub kind: TagKind,
    pub writable: bool,
    pub range: Option<(f64, f64)>,
    pub singleton: bool, // true = 不参与 count 展开（_System.Time）
}

/// 8 个展开类型 + 1 单例（顺序无关，browse 树按名字）。
pub(crate) static TYPES: &[TagType] = &[
    // 注意：VT_I4=3, VT_R8=5, VT_BOOL=11（windows::Win32::System::Variant 值）
    TagType { prefix: "Random.Int4",        dtype: 3,  kind: TagKind::Random,   writable: false, range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "Random.Real8",       dtype: 5,  kind: TagKind::Random,   writable: false, range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "Square.Real8",       dtype: 5,  kind: TagKind::Square,   writable: false, range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "Sawtooth.Real8",     dtype: 5,  kind: TagKind::Sawtooth, writable: false, range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "Triangle.Real8",     dtype: 5,  kind: TagKind::Triangle, writable: false, range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "BucketBrigade.Int4", dtype: 3,  kind: TagKind::Counter,  writable: true,  range: Some((0.0, 100.0)), singleton: false },
    TagType { prefix: "WriteTag.Int4",      dtype: 3,  kind: TagKind::Register, writable: true,  range: None,                singleton: false },
    TagType { prefix: "AltBool.Bool",       dtype: 11, kind: TagKind::AltBool,  writable: false, range: None,                singleton: false },
    TagType { prefix: "_System.Time",       dtype: 5,  kind: TagKind::SysTime,  writable: false, range: None,                singleton: true  },
];

/// 展开所有 item_id：每个非 singleton 类型生成 count 个 `{prefix}.{i}`，singleton 加 prefix 本身。
pub(crate) fn expand_ids(count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for t in TYPES {
        if t.singleton {
            ids.push(t.prefix.to_string());
        } else {
            for i in 0..count {
                ids.push(format!("{}.{}", t.prefix, i));
            }
        }
    }
    ids
}

/// 按 '.' 分割 ids，Trie 式合并公共前缀为 hierarchical `NsNode` 树（root 为空名 Branch）。
pub(crate) fn build_namespace_tree(ids: &[String]) -> NsNode {
    let mut root = NsNode::Branch { name: Arc::from(""), children: Vec::new() };
    for id in ids {
        let parts: Vec<&str> = id.split('.').collect();
        insert_path(&mut root, &parts, id);
    }
    root
}

/// 递归插入一条路径（parts 为按 '.' 切片，full_id 为完整 item_id 用于 Leaf）。
fn insert_path(node: &mut NsNode, parts: &[&str], full_id: &str) {
    let children = match node {
        NsNode::Branch { children, .. } => children,
        NsNode::Leaf { .. } => return,
    };
    if parts.len() == 1 {
        children.push(NsNode::Leaf { id: Arc::from(full_id) });
        return;
    }
    let head = parts[0];
    let pos = children.iter().position(|c| match c {
        NsNode::Branch { name, .. } => name.as_ref() == head,
        NsNode::Leaf { .. } => false,
    });
    let new_child_idx = match pos {
        Some(i) => i,
        None => {
            children.push(NsNode::Branch { name: Arc::from(head), children: Vec::new() });
            children.len() - 1
        }
    };
    insert_path(&mut children[new_child_idx], &parts[1..], full_id);
}

#[cfg(test)]
mod tests {
    // ...（Step 2 的测试放这里）
}
```

> 注：`dtype` 用 `u16` 存 VARENUM 数值（VT_I4=3 / VT_R8=5 / VT_BOOL=11），避免在 tags.rs 顶部 import windows VARIANT 常量；data_source.rs 用时再 `VARENUM::from(...)` 或直接比较。

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p opc-da-server-sim tags`
Expected: 5 个测试全 PASS。

- [ ] **Step 6: Commit**

```bash
git add opc-da-server-sim/src/tags.rs opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): 类型表 + count 展开 + hierarchical 建树"
```

---

## Task 4: data_source.rs（SimDataSource + VARIANT helpers + read/write）

**Files:**
- Create: `opc-da-server-sim/src/data_source.rs`
- Modify: `opc-da-server-sim/src/main.rs`（加 `mod data_source;`）

- [ ] **Step 1: main.rs 加 mod 声明**

```rust
#[cfg(windows)]
mod data_source;
```

- [ ] **Step 2: 写失败测试**

创建 `src/data_source.rs`，先写测试：

```rust
//! SimDataSource——实现 DataSource trait，按类型表 + count 产生 tag。

use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, FILETIME, S_OK};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_BOOL, VT_I4, VT_R8};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::TagKind; // 仅测试用，确认类型存在

    #[test]
    fn namespace_size_and_org() {
        let ds = SimDataSource::new(5);
        assert_eq!(ds.namespace().leaves().len(), 8 * 5 + 1, "41 tag");
        assert_eq!(ds.query_organization(), opc_da_server::data_source::NsOrganization::Hierarchical);
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
        assert_eq!(ds.write("BucketBrigade.Int4.0", &variant_r8(1.0)), E_INVALIDARG);
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

    /// 测试辅助：读 VARIANT 的 vt。
    fn vt_of(v: &VARIANT) -> VARENUM {
        // SAFETY: 只读 vt 判别字段；v 是自构造有效 VARIANT。
        unsafe { (*v.Anonymous.Anonymous).vt }
    }
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p opc-da-server-sim data_source`
Expected: 编译失败（`SimDataSource` / `OPC_QUALITY_GOOD` / `variant_i4` 等未定义）。

- [ ] **Step 4: 写实现**

在 `src/data_source.rs` 测试 mod 之前加完整实现：

```rust
//! SimDataSource——实现 DataSource trait，按类型表 + count 产生 tag。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opc_da_server::data_source::{
    DataSource, ItemMeta, NamespaceTree, NsNode, NsOrganization, OPC_QUALITY_BAD,
    OPC_QUALITY_GOOD,
};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, FILETIME, S_OK};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_BOOL, VT_I4, VT_R8};
use windows::core::HRESULT;
use windows::core::imp::BOOL; // 见下注：用 windows::Win32::Foundation::BOOL

use crate::tags::{TagKind, TYPES, build_namespace_tree, expand_ids};
use crate::waveform;

// —— VARIANT helpers（复制库 data_source.rs 的 pub(crate) 实现；sim 无法引用库私有项）——

/// 构造 VT_I4 VARIANT。
fn variant_i4(value: i32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 设 vt 判别 + union 字段 lVal；var 按值返回，无别名。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_I4;
        (*var.Anonymous.Anonymous).Anonymous.lVal = value;
    }
    var
}

/// 构造 VT_R8 VARIANT。
fn variant_r8(value: f64) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 同 variant_i4；dblVal 为 f64 union 字段。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_R8;
        (*var.Anonymous.Anonymous).Anonymous.dblVal = value;
    }
    var
}

/// 构造 VT_BOOL VARIANT。
fn variant_bool(value: bool) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: 同 variant_i4；boolVal 为 BOOL union 字段。
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BOOL;
        (*var.Anonymous.Anonymous).Anonymous.boolVal = BOOL::from(value);
    }
    var
}

/// 解析 VT_I4 VARIANT → i32；类型不符返 None。
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

/// 当前 UTC 时间 → FILETIME（复制库 now_filetime）。
fn now_filetime() -> FILETIME {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
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

// —— SimDataSource ——

/// 示例仿真数据源：8 类型 × count + _System.Time，hierarchical，read-time 纯计算。
pub struct SimDataSource {
    ns: NamespaceTree,
    start: Instant,
    leaves: HashSet<String>,
    /// 可写类型的寄存器（按展开顺序，index 对应 item_id 的尾段）。
    counter_regs: Vec<AtomicI32>,
    write_regs: Vec<AtomicI32>,
}

impl SimDataSource {
    /// 新建：count = 每类型实例数（1..=100_000，caller 负责 clamp）。
    #[must_use]
    pub fn new(count: usize) -> Self {
        let ids = expand_ids(count);
        let root = build_namespace_tree(&ids);
        let ns = NamespaceTree::from_tree(root);
        let leaves: HashSet<String> = ids.iter().cloned().collect();
        Self {
            ns,
            start: Instant::now(),
            leaves,
            counter_regs: vec_with_atomic(count),
            write_regs: vec_with_atomic(count),
        }
    }

    /// 解析 item_id → (类型索引 in TYPES, index)。失败返 None。
    fn parse(&self, item_id: &str) -> Option<(usize, usize)> {
        for (ti, t) in TYPES.iter().enumerate() {
            if t.singleton {
                if item_id == t.prefix {
                    return Some((ti, 0));
                }
            } else if let Some(rest) = item_id.strip_prefix(t.prefix) {
                // prefix 后必须是 '.' + 数字。
                if let Some(num_str) = rest.strip_prefix('.') {
                    if let Ok(idx) = num_str.parse::<usize>() {
                        return Some((ti, idx));
                    }
                }
            }
        }
        None
    }
}

/// 构造 count 个 AtomicI32（初值 0）的 Vec。
fn vec_with_atomic(count: usize) -> Vec<AtomicI32> {
    (0..count).map(|_| AtomicI32::new(0)).collect()
}

impl DataSource for SimDataSource {
    fn namespace(&self) -> &NamespaceTree {
        &self.ns
    }

    fn read(&self, item_id: &str) -> (VARIANT, u16, FILETIME) {
        let ts = now_filetime();
        let Some((ti, idx)) = self.parse(item_id) else {
            return (VARIANT::default(), OPC_QUALITY_BAD, ts);
        };
        let t = &TYPES[ti];
        let elapsed_secs = self.start.elapsed().as_secs();
        match t.kind {
            TagKind::Counter => {
                let v = self.counter_regs.get(idx).map_or(0, |a| a.load(Ordering::Relaxed));
                (variant_i4(v), OPC_QUALITY_GOOD, ts)
            }
            TagKind::Register => {
                let v = self.write_regs.get(idx).map_or(0, |a| a.load(Ordering::Relaxed));
                (variant_i4(v), OPC_QUALITY_GOOD, ts)
            }
            _ => {
                let raw = waveform::value(t.kind, idx as u64, elapsed_secs);
                let var = match t.dtype {
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
        let Some((ti, idx)) = self.parse(item_id) else {
            return E_ACCESSDENIED; // 未知 item 视为不可写
        };
        let t = &TYPES[ti];
        if !t.writable {
            return E_ACCESSDENIED;
        }
        let Some(i) = variant_as_i4(value) else {
            return E_INVALIDARG; // 非 VT_I4
        };
        let regs = match t.kind {
            TagKind::Counter => &self.counter_regs,
            TagKind::Register => &self.write_regs,
            _ => return E_ACCESSDENIED, // 不可写类型（理论不会到这，t.writable 已挡）
        };
        if let Some(slot) = regs.get(idx) {
            slot.store(i, Ordering::Relaxed);
            S_OK
        } else {
            E_INVALIDARG // index 越界
        }
    }

    fn item_meta(&self, item_id: &str) -> Option<ItemMeta> {
        let (ti, _) = self.parse(item_id)?;
        let t = &TYPES[ti];
        Some(ItemMeta {
            data_type: VARENUM(t.dtype),
            writable: t.writable,
        })
    }

    fn item_range(&self, item_id: &str) -> Option<(f64, f64)> {
        let (ti, _) = self.parse(item_id)?;
        TYPES[ti].range
    }

    fn query_organization(&self) -> NsOrganization {
        NsOrganization::Hierarchical
    }

    // browse_branch 用默认实现（委托 namespace().browse_children）。
    let _ = NsNode::Branch { name: Arc::from(""), children: vec![] }; // 抑制未用 import
}

#[cfg(test)]
mod tests {
    // ...（Step 2 的测试放这里，含 vt_of 辅助）
}
```

> **Step 4 注意事项（实现时逐条核对）：**
> 1. `BOOL` import：用 `use windows::Win32::Foundation::BOOL;`（**不是** `windows::core::imp::BOOL`——上面占位 import 是错的，改成 Foundation）。删除占位行 `use windows::core::imp::BOOL;` 和 `let _ = NsNode::...` 那行（那两行是为提示，实现时删除）。
> 2. `VARENUM(t.dtype)`：windows-rs 的 `VARENUM` 是 `#[repr(...)]` 可从 u16 构造——若编译器要求 `VARENUM(t.dtype)` 不行，改用 `VARENUM::from_u16(t.dtype)` 或 `unsafe { std::mem::transmute(t.dtype) }`。**以编译器实际接受为准**，优先 `VARENUM::from(t.dtype)` 若有，否则直接 `t.dtype as VARENUM`（`VARENUM` 是 unit struct + associated constant 风格时需特殊处理）。验证命令见 Step 5。
> 3. 删除 `let _ = NsNode::Branch { .. };` 行（仅用于避免 import 警告，实际 browse_branch 用默认 trait 方法，不需要 NsNode）。

- [ ] **Step 5: 跑测试 + 修 VARENUM/BOOL 编译问题**

Run: `cargo test -p opc-da-server-sim data_source`
Expected: 编译可能因 `VARENUM`/`BOOL` 构造方式报错。按编译器提示修：
- `BOOL`：确保 `use windows::Win32::Foundation::BOOL;`，`BOOL::from(value)`。
- `VARENUM`：先试 `VARENUM(t.dtype)`；不行试 `t.dtype as VARENUM`（`VARENUM` 在 windows-rs 0.61 是 `#[repr(transparent)]` newtype over u16，`as` 转换合法）。
- 修完后重跑，预期 12 个测试全 PASS。

- [ ] **Step 6: clippy**

Run: `cargo clippy -p opc-da-server-sim --all-targets -- -D warnings`
Expected: 无 warning（每个 unsafe 块有 `// SAFETY:`）。

- [ ] **Step 7: Commit**

```bash
git add opc-da-server-sim/src/data_source.rs opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): SimDataSource 实现 + read/write 单测"
```

---

## Task 5: runtime.rs（CLSID/ProgID + 注册 + run 编排）

**Files:**
- Create: `opc-da-server-sim/src/runtime.rs`
- Modify: `opc-da-server-sim/src/main.rs`（加 `mod runtime;`）

- [ ] **Step 1: main.rs 加 mod 声明**

```rust
#[cfg(windows)]
mod runtime;
```

- [ ] **Step 2: 写 build_registration 单测 + 实现**

创建 `src/runtime.rs`：

```rust
//! COM 编排：CLSID/ProgID 常量 + build_registration + run（复制库 bin 模板）。

use std::path::Path;
use std::time::Duration;

use opc_da_client::bindings::da::{CATID_OPCDAServer10, CATID_OPCDAServer20, CATID_OPCDAServer30};
use opc_da_server::class_factory::Factory;
use opc_da_server::data_source::DataSource;
use opc_da_server::objects::scheduler;
use opc_da_server::registry::{ServerRegistration, register, unregister};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoIncrementMTAUsage, CoInitializeSecurity, CoRegisterClassObject,
    CoResumeClassObjects, EOAC_NONE, IClassFactory, REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
    RPC_C_AUTHN_LEVEL_CONNECT, RPC_C_IMP_LEVEL_IDENTIFY,
};
use windows::core::{GUID, Result};

use crate::data_source::SimDataSource;

/// sim 的独立 CLSID（与库 CLSID_OPC_DA_SERVER 0x9a7b_3c2d_... 不同）。
pub(crate) const CLSID_OPC_DA_SIM: GUID = GUID::from_u128(0xb1c2_d3e4_f5a6_0718_293a_4b5c_5d6e_7f80);
const PROG_ID: &str = "opc-da-rs.Sim.1";
const VIPROG_ID: &str = "opc-da-rs.Sim";
const DESCRIPTION: &str = "opc-da-rs OPC DA Simulation Server";

const CATIDS: [GUID; 3] = [
    CATID_OPCDAServer10::IID,
    CATID_OPCDAServer20::IID,
    CATID_OPCDAServer30::IID,
];

/// 构造注册参数（/RegServer 与 /UnregServer 共用）。
fn build_registration(exe_path: &Path) -> ServerRegistration<'_> {
    ServerRegistration {
        clsid: CLSID_OPC_DA_SIM,
        prog_id: PROG_ID,
        version_independent_prog_id: VIPROG_ID,
        exe_path,
        catids: &CATIDS,
        app_id: CLSID_OPC_DA_SIM,
        description: DESCRIPTION,
    }
}

/// 读 env OPC_DA_SIM_COUNT（默认 100，clamp 1..=100_000）。
fn read_count() -> usize {
    std::env::var("OPC_DA_SIM_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| (1..=100_000).contains(&n))
        .unwrap_or(100)
}

/// /RegServer：写 HKCR 注册项后退出（需管理员）。
pub fn run_register() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    register(&reg)?;
    eprintln!("opc-da-server-sim: registered (ProgID={})", reg.prog_id);
    Ok(())
}

/// /UnregServer：递归清 HKCR（64+32 视图，幂等）。
pub fn run_unregister() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    unregister(&reg)?;
    eprintln!("opc-da-server-sim: unregistered (ProgID={})", reg.prog_id);
    Ok(())
}

/// 服务循环：注册类对象 + 阻塞（Ctrl+C 终止）。
pub fn run_server() -> Result<()> {
    let count = read_count();
    // SAFETY: 标准 EXE server 启动序列（复制 opc-da-server/src/bin/opc-da-server.rs:83-119）。
    unsafe {
        CoIncrementMTAUsage()?;
        let workers = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
        scheduler::init(workers);
        // SAFETY: CoInitializeSecurity 在 COM 初始化后 + 首次激活前调；cauthn=-1 让 COM 选认证服务。
        CoInitializeSecurity(
            None, -1, None, None,
            RPC_C_AUTHN_LEVEL_CONNECT,
            RPC_C_IMP_LEVEL_IDENTIFY,
            None, EOAC_NONE, None,
        )?;
        let ds: std::sync::Arc<dyn DataSource> = std::sync::Arc::new(SimDataSource::new(count));
        let factory: IClassFactory = Factory::new(ds).into();
        let _cookie = CoRegisterClassObject(
            &CLSID_OPC_DA_SIM,
            &factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED,
        )?;
        CoResumeClassObjects()?;
        eprintln!(
            "opc-da-server-sim: serving (ProgID={}, {} tags, Ctrl+C 退出)",
            PROG_ID,
            8 * count + 1
        );
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_fields() {
        let reg = build_registration(Path::new("C:\\test.exe"));
        assert_eq!(reg.prog_id, "opc-da-rs.Sim.1");
        assert_eq!(reg.version_independent_prog_id, "opc-da-rs.Sim");
        assert_eq!(reg.clsid, CLSID_OPC_DA_SIM);
        assert_ne!(reg.clsid, opc_da_server::class_factory::CLSID_OPC_DA_SERVER, "必须与库 CLSID 不同");
        assert_eq!(reg.description, DESCRIPTION);
        assert_eq!(reg.catids.len(), 3);
    }
}
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p opc-da-server-sim runtime`
Expected: `registration_fields` PASS（验证 ProgID/CLSID 字段 + CLSID 与库不同）。

- [ ] **Step 4: clippy**

Run: `cargo clippy -p opc-da-server-sim --all-targets -- -D warnings`
Expected: 无 warning。

- [ ] **Step 5: Commit**

```bash
git add opc-da-server-sim/src/runtime.rs opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): runtime COM 编排 + 注册 + build_registration 单测"
```

---

## Task 6: main.rs（args 解析 + 装配）

**Files:**
- Modify: `opc-da-server-sim/src/main.rs`（替换 Task 1 的占位 main）

- [ ] **Step 1: 替换 main.rs 全文**

```rust
//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（需管理员），注册后退出。
//! - `/UnregServer` 清注册项（幂等）。
//! - 无参          启动服务循环。tag 数量由 env `OPC_DA_SIM_COUNT` 控制（默认 100）。
//!
//! Windows-only（依赖 opc-da-server 的 COM 实现）。非 Windows 由库 compile_error! 拒编译。

#[cfg(windows)]
mod data_source;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod tags;
#[cfg(windows)]
mod waveform;

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a.eq_ignore_ascii_case("/RegServer")) {
        return runtime::run_register();
    }
    if args.iter().any(|a| a.eq_ignore_ascii_case("/UnregServer")) {
        return runtime::run_unregister();
    }
    runtime::run_server()
}

#[cfg(not(windows))]
fn main() {
    // 非 Windows：opc-da-server 库已 compile_error!，这里不可达。
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build -p opc-da-server-sim`
Expected: 编译成功。

- [ ] **Step 3: 手测注册（需管理员终端）**

在**管理员**终端运行：
```
cargo run -p opc-da-server-sim --release -- /RegServer
```
Expected: 输出 `opc-da-server-sim: registered (ProgID=opc-da-rs.Sim.1)`，退出码 0。

验证注册表（PowerShell）：
```
Get-ItemProperty "HKCR:\opc-da-rs.Sim.1" -Name "(default)"
```
Expected: 默认值 = `opc-da-rs OPC DA Simulation Server`。

- [ ] **Step 4: 手测反注册**

```
cargo run -p opc-da-server-sim --release -- /UnregServer
```
Expected: 输出 `opc-da-server-sim: unregistered`，退出码 0。`Get-ItemProperty` 应报键不存在。

- [ ] **Step 5: 手测运行（先 /RegServer，再启动）**

```
cargo run -p opc-da-server-sim --release -- /RegServer   # 管理员，注册
$env:OPC_DA_SIM_COUNT = "1000"
cargo run -p opc-da-server-sim --release                 # 启动，应输出 "8001 tags"
```
Expected: 输出 `serving (ProgID=opc-da-rs.Sim.1, 8001 tags, Ctrl+C 退出)` 并阻塞。Ctrl+C 终止。

- [ ] **Step 6: Commit**

```bash
git add opc-da-server-sim/src/main.rs
git commit -m "feat(opc-da-server-sim): main 装配 + args 解析 + 注册/运行"
```

---

## Task 7: client-test 加 ProgID 覆盖 + e2e 连 sim

**Files:**
- Modify: `opc-da-client-test/src/server_proc.rs`
- Modify: `opc-da-client-test/src/e2e.rs`
- Modify: `opc-da-client-test/src/stress.rs`

- [ ] **Step 1: server_proc.rs 透传 ProgID env**

读 `opc-da-client-test/src/server_proc.rs`，找到 `ServerChild::spawn` 里设 `OPC_DA_DATASOURCE` 的地方（约 `:30`），在 env 设置块中追加：
```rust
// 透传 ProgID 覆盖（默认 opc-da-rs.Server.1；e2e 连 sim 时设 opc-da-rs.Sim.1）。
if let Ok(v) = std::env::var("OPC_DA_SERVER_PROGID") {
    command.env("OPC_DA_SERVER_PROGID", v);
}
```
（精确行号/写法以源码为准——先 Read 该文件确认 `command.env(...)` 模式，照搬。）

- [ ] **Step 2: e2e.rs / stress.rs 的 PROG_ID 读 env**

读 `e2e.rs:16` 与 `stress.rs:20`，把：
```rust
const PROG_ID: &str = "opc-da-rs.Server.1";
```
改为：
```rust
fn prog_id() -> String {
    std::env::var("OPC_DA_SERVER_PROGID").unwrap_or_else(|_| "opc-da-rs.Server.1".to_string())
}
```
并把所有 `PROG_ID` 引用点改为 `prog_id()`（或 `.as_str()` 视签名）。

- [ ] **Step 3: 验证默认行为不回归**

Run: `cargo test -p opc-da-client-test`（无 env，默认连 opc-da-rs.Server.1）
Expected: 原有 e2e 全过（client-test 默认 spawn 库 bin，ProgID 默认）。

> ⚠ 注意：client-test 默认 spawn 的是 `opc-da-server` bin（`OPC_DA_DATASOURCE` 切数据源），不是 sim。默认路径不变。sim 路径需手动设 `OPC_DA_SERVER_PROGID=opc-da-rs.Sim.1` 并确保 sim 已 `/RegServer`。

- [ ] **Step 4: e2e 连 sim（需先注册 sim）**

管理员终端：
```
cargo run -p opc-da-server-sim --release -- /RegServer
```
然后：
```
$env:OPC_DA_SERVER_PROGID = "opc-da-rs.Sim.1"
cargo run -p opc-da-client-test --release
```
Expected: client-test 启动 sim 子进程（经 SCM），e2e 跑通 sim 全 tag 的 browse/read/write/subscribe。17 探针全过（sim 的 namespace 与库 sim 4-tag 不同，但 client-test e2e 若硬编码 `Random.Int4` 等具体 tag 名，需确认 sim 的 `Random.Int4.0` 等能被匹配——**若 e2e 硬编码了 4 个 tag 名，需在 e2e.rs 适配为 `.0` 后缀，或在 sim 加无后缀别名**。先 Read `e2e.rs` 确认断言的 tag 名，按需调整）。

> 这是本计划最可能踩坑的点：client-test e2e 断言的 tag 名（`Random.Int4` 等）与 sim 的带后缀名（`Random.Int4.0`）不一致。**实现时先 Read `e2e.rs:150-180` 看断言**，若冲突，方案有二：(a) 改 e2e 断言用 `Random.Int4.0`；(b) sim 对 count==... 不加后缀。推荐 (a)（sim 的命名约定不变）。

- [ ] **Step 5: stress 连 sim 验规模化**

```
$env:OPC_DA_SERVER_PROGID = "opc-da-rs.Sim.1"
$env:OPC_DA_SIM_COUNT = "10000"
cargo run -p opc-da-client-test --release -- stress --clients 10 --duration 60
```
Expected: 10 client × 80001 tag 订阅 60s，无报错，RSS 稳态（参考 scale-plan §10 基线）。

- [ ] **Step 6: Commit**

```bash
git add opc-da-client-test/src/server_proc.rs opc-da-client-test/src/e2e.rs opc-da-client-test/src/stress.rs
git commit -m "feat(opc-da-client-test): 支持 OPC_DA_SERVER_PROGID 覆盖 + e2e 连 sim"
```

---

## Task 8: README + 全质量门

**Files:**
- Create: `opc-da-server-sim/README.md`

- [ ] **Step 1: 写 README（仿 opc-da-desktop/README.md 结构）**

包含：项目简介、架构树（5 个 src 文件）、功能列表、构建/注册/运行/反注册命令、`OPC_DA_SIM_COUNT` 用法、tag 类型表、已知限制（Ctrl+C 退出、SCM 自动启动不带 env、远程 DCOM 需 OPCproxy.dll）、与 opc-da-server 库的关系。

- [ ] **Step 2: 全质量门**

Run: `pwsh -File scripts/verify.ps1`
Expected: fmt → clippy(-D warnings) → test-doc → test-workspace → compat 逐个构建，全过，退出码 0。

- [ ] **Step 3: Commit**

```bash
git add opc-da-server-sim/README.md
git commit -m "docs(opc-da-server-sim): README + 全质量门通过"
```

---

## Self-Review

**1. Spec coverage:**
- §1 目标（独立 crate / 薄包装 / 8×count+单例 / env count / 大规模订阅 / 可写 tag）→ Task 1-6 全覆盖
- §4 DataSource（类型表 + 建树 + read/write 分派）→ Task 3+4
- §5 值生成器公式 → Task 2（Random/Square/Sawtooth/Triangle/AltBool/SysTime）+ Task 4（Counter/Register）
- §6 注册/运行/退出 → Task 5+6
- §7 env 配置 → Task 5（read_count）
- §8 约束（panic/unsafe/MTA）→ 各 Task 的 clippy 步骤
- §9 验收（质量门 + 单测 + e2e + stress）→ Task 2-7 单测 / Task 7 e2e+stress / Task 8 质量门
- §10 风险 → Task 6 手测 + README（Task 8）

**2. Placeholder scan:** 无 TBD/TODO。Task 4 Step 4 的"VARENUM/BOOL 构造以编译器为准"是诚实标注的不确定性（两种候选写法），不是占位——实现者按编译器提示选其一。Task 7 Step 4 的 e2e tag 名适配同理（先 Read 再定）。

**3. Type consistency:**
- `TagKind`：Task 2 定义（8 变体），Task 3 `TagType.kind` 用，Task 4 `read/write` match —— 一致
- `TagType`：Task 3 定义（prefix/dtype/kind/writable/range/singleton），Task 4 `TYPES[ti]` 访问 —— 一致
- `SimDataSource::new(count)`：Task 4 定义，Task 5 `run_server` 调用 —— 一致
- `CLSID_OPC_DA_SIM`：Task 5 定义，Task 5 测试 + Task 6 注册用 —— 一致
- `dtype: u16`（Task 3）→ `VARENUM(t.dtype)`（Task 4）：需 Step 5 确认构造方式

**4. 已知风险点（实现时重点关注）：**
- Task 4 `VARENUM` 构造（windows-rs 0.61 newtype）
- Task 4 `BOOL` import 路径（`Win32::Foundation::BOOL`）
- Task 7 e2e 断言的 tag 名与 sim 带后缀名是否冲突

---

## Execution Handoff

计划已保存到 `docs/superpowers/plans/2026-08-04-opc-da-server-sim.md`。
