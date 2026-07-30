/**
 * TagBrowserModal — left branch tree + right leaf list, multi-select.
 *
 * Backed by the lazy `browse_children` IPC: opening the modal loads the
 * root's children; clicking a branch's ▸ expands it (one round-trip per
 * node). Selecting a branch (anywhere on its row) shows its leaves on the
 * right, where individual tags can be checked.
 *
 * Matrikon Simulation is hierarchical (Random / Bucket Brigade / … branches,
 * each holding leaf tags), which is exactly what this tree renders.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useConnectionStore } from "../stores/connection";
import { browseChildren, type BranchNode, type LeafNode } from "../api/tauri";

const ROOT_KEY = "__root__";
const branchKey = (id: string | null) => (id === null ? ROOT_KEY : id);

interface Props {
    onAdd: (tagIds: string[]) => void;
    onClose: () => void;
}

export function TagBrowserModal({ onAdd, onClose }: Props) {
    const progId = useConnectionStore((s) => s.progId);

    const [roots, setRoots] = useState<BranchNode[]>([]);
    const [childBranches, setChildBranches] = useState<Map<string, BranchNode[]>>(new Map());
    const [leavesByBranch, setLeavesByBranch] = useState<Map<string, LeafNode[]>>(new Map());
    const [expanded, setExpanded] = useState<Set<string>>(new Set());
    const [selectedBranch, setSelectedBranch] = useState<string | null>(null);
    const [selectedLeaves, setSelectedLeaves] = useState<Set<string>>(new Set());
    const [leafFilter, setLeafFilter] = useState("");
    const [error, setError] = useState<string | null>(null);

    // Load root children on open.
    useEffect(() => {
        setError(null);
        browseChildren(null)
            .then((c) => {
                setRoots(c.branches);
                setLeavesByBranch((m) => new Map(m).set(ROOT_KEY, c.leaves));
            })
            .catch((e) => setError(String(e)));
    }, [progId]);

    const expand = useCallback(
        async (branch: BranchNode) => {
            if (childBranches.has(branch.id)) {
                setExpanded((s) => {
                    const next = new Set(s);
                    if (next.has(branch.id)) next.delete(branch.id);
                    else next.add(branch.id);
                    return next;
                });
                return;
            }
            try {
                const c = await browseChildren(branch.id);
                setChildBranches((m) => new Map(m).set(branch.id, c.branches));
                setLeavesByBranch((m) => new Map(m).set(branch.id, c.leaves));
                setExpanded((s) => new Set(s).add(branch.id));
            } catch (e) {
                setError(String(e));
            }
        },
        [childBranches],
    );

    const currentLeaves = leavesByBranch.get(branchKey(selectedBranch)) ?? [];
    const visibleLeaves = useMemo(() => {
        const f = leafFilter.trim().toLowerCase();
        if (!f) return currentLeaves;
        return currentLeaves.filter(
            (l) =>
                l.name.toLowerCase().includes(f) || l.item_id.toLowerCase().includes(f),
        );
    }, [currentLeaves, leafFilter]);

    const toggleLeaf = (itemId: string) => {
        setSelectedLeaves((s) => {
            const next = new Set(s);
            if (next.has(itemId)) next.delete(itemId);
            else next.add(itemId);
            return next;
        });
    };

    const renderBranch = (branch: BranchNode, depth: number) => {
        const isExpanded = expanded.has(branch.id);
        const isSelected = selectedBranch === branch.id;
        const kids = childBranches.get(branch.id) ?? [];
        return (
            <div key={branch.id}>
                <div
                    className={`tree-node ${isSelected ? "selected" : ""}`}
                    style={{ paddingLeft: 8 + depth * 14 }}
                    onClick={() => setSelectedBranch(branch.id)}
                >
                    <span
                        className="tree-twisty"
                        onClick={(e) => {
                            e.stopPropagation();
                            void expand(branch);
                        }}
                    >
                        {isExpanded ? "▾" : "▸"}
                    </span>
                    📁 {branch.name}
                </div>
                {isExpanded && kids.map((k) => renderBranch(k, depth + 1))}
            </div>
        );
    };

    return (
        <div className="modal-backdrop" onClick={onClose}>
            <div className="modal" onClick={(e) => e.stopPropagation()}>
                <div className="modal-header">Add Tags from {progId ?? "(not connected)"}</div>
                {error && <div style={{ padding: 8, color: "#f48771" }}>{error}</div>}
                <div className="modal-body">
                    <div className="tree">
                        <div
                            className={`tree-node ${selectedBranch === null ? "selected" : ""}`}
                            style={{ paddingLeft: 8 }}
                            onClick={() => setSelectedBranch(null)}
                        >
                            <span className="tree-twisty"> </span>
                            🌐 {progId ?? "(root)"}
                        </div>
                        {roots.map((b) => renderBranch(b, 1))}
                    </div>
                    <div className="leaf-pane">
                        <div style={{ padding: "4px 8px", borderBottom: "1px solid #3c3c3c" }}>
                            <input
                                type="text"
                                placeholder="filter leaves…"
                                value={leafFilter}
                                onChange={(e) => setLeafFilter(e.target.value)}
                                style={{ width: "100%", padding: "2px 6px" }}
                            />
                        </div>
                        <div className="leaf-list">
                            {visibleLeaves.length === 0 && (
                                <div style={{ padding: 8, color: "#858585", fontSize: 11 }}>
                                    No tags under this node.
                                </div>
                            )}
                            {visibleLeaves.map((l) => (
                                <div
                                    key={l.item_id}
                                    className={`leaf ${selectedLeaves.has(l.item_id) ? "selected" : ""}`}
                                    onClick={() => toggleLeaf(l.item_id)}
                                    title={l.item_id}
                                >
                                    {selectedLeaves.has(l.item_id) ? "☑" : "☐"} {l.name}
                                </div>
                            ))}
                        </div>
                    </div>
                </div>
                <div className="modal-footer">
                    <span style={{ marginRight: "auto", alignSelf: "center", fontSize: 11 }}>
                        {selectedLeaves.size} selected
                    </span>
                    <button onClick={onClose}>Cancel</button>
                    <button
                        onClick={() => onAdd(Array.from(selectedLeaves))}
                        disabled={selectedLeaves.size === 0}
                    >
                        Add {selectedLeaves.size > 0 ? `(${selectedLeaves.size})` : ""}
                    </button>
                </div>
            </div>
        </div>
    );
}
