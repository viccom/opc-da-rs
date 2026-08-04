//! tag 类型表 + count 展开 + 按 '.' 建 hierarchical 命名空间树。

use std::sync::Arc;

use opc_da_server::data_source::NsNode;

use crate::waveform::TagKind;

/// 单个 tag 类型的定义。
///
/// `dtype`/`kind`/`writable`/`range` 在本 task 内仅声明未读——Task 4（`data_source.rs`）的
/// `read`/`item_meta` 分流时消费。在此之前 dead-code 检查会误报，故显式 allow。
#[allow(dead_code)]
pub struct TagType {
    pub prefix: &'static str,
    pub dtype: u16, // VARENUM（VT_I4=3 / VT_R8=5 / VT_BOOL=11）
    pub kind: TagKind,
    pub writable: bool,
    pub range: Option<(f64, f64)>,
    pub singleton: bool, // true = 不参与 count 展开（_System.Time）
}

/// 8 个展开类型 + 1 单例。
#[allow(dead_code)]
pub static TYPES: &[TagType] = &[
    TagType {
        prefix: "Random.Int4",
        dtype: 3,
        kind: TagKind::Random,
        writable: false,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "Random.Real8",
        dtype: 5,
        kind: TagKind::Random,
        writable: false,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "Square.Real8",
        dtype: 5,
        kind: TagKind::Square,
        writable: false,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "Sawtooth.Real8",
        dtype: 5,
        kind: TagKind::Sawtooth,
        writable: false,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "Triangle.Real8",
        dtype: 5,
        kind: TagKind::Triangle,
        writable: false,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "BucketBrigade.Int4",
        dtype: 3,
        kind: TagKind::Counter,
        writable: true,
        range: Some((0.0, 100.0)),
        singleton: false,
    },
    TagType {
        prefix: "WriteTag.Int4",
        dtype: 3,
        kind: TagKind::Register,
        writable: true,
        range: None,
        singleton: false,
    },
    TagType {
        prefix: "AltBool.Bool",
        dtype: 11,
        kind: TagKind::AltBool,
        writable: false,
        range: None,
        singleton: false,
    },
    TagType {
        prefix: "_System.Time",
        dtype: 5,
        kind: TagKind::SysTime,
        writable: false,
        range: None,
        singleton: true,
    },
];

/// 展开所有 item_id：非 singleton 类型生成 count 个 `{prefix}.{i}`，singleton 加 prefix 本身。
#[allow(dead_code)]
pub fn expand_ids(count: usize) -> Vec<String> {
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
#[allow(dead_code)]
pub fn build_namespace_tree(ids: &[String]) -> NsNode {
    let mut root = NsNode::Branch {
        name: Arc::from(""),
        children: Vec::new(),
    };
    for id in ids {
        let parts: Vec<&str> = id.split('.').collect();
        insert_path(&mut root, &parts, id);
    }
    root
}

#[allow(dead_code)]
fn insert_path(node: &mut NsNode, parts: &[&str], full_id: &str) {
    let children = match node {
        NsNode::Branch { children, .. } => children,
        NsNode::Leaf { .. } => return,
    };
    if parts.len() == 1 {
        children.push(NsNode::Leaf {
            id: Arc::from(full_id),
        });
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
            children.push(NsNode::Branch {
                name: Arc::from(head),
                children: Vec::new(),
            });
            children.len() - 1
        }
    };
    insert_path(&mut children[new_child_idx], &parts[1..], full_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_default_count() {
        let ids = expand_ids(100);
        assert_eq!(ids.len(), 8 * 100 + 1, "默认 801 tag");
        assert!(ids.contains(&"_System.Time".to_string()), "含单例");
        assert!(ids.contains(&"Random.Int4.0".to_string()));
        assert!(ids.contains(&"Random.Int4.99".to_string()));
        assert!(
            !ids.contains(&"Random.Int4.100".to_string()),
            "index 上限 99"
        );
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
        let mut sorted = ids;
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
        let names: Vec<&str> = children
            .iter()
            .filter_map(|c| match c {
                NsNode::Branch { name, .. } => Some(name.as_ref()),
                NsNode::Leaf { .. } => None,
            })
            .collect();
        assert!(names.contains(&"Random"), "缺 Random 分支");
        assert!(names.contains(&"_System"), "缺 _System 分支");
    }

    #[test]
    fn tree_random_int4_has_3_leaves() {
        let ids = expand_ids(3);
        let root = build_namespace_tree(&ids);
        let n = opc_da_server::data_source::NamespaceTree::from_tree(root);
        assert_eq!(
            n.browse_children(&["Random"]).len(),
            2,
            "Random 下 Int4/Real8"
        );
        assert_eq!(
            n.browse_children(&["Random", "Int4"]).len(),
            3,
            "3 个 index 叶"
        );
    }
}
