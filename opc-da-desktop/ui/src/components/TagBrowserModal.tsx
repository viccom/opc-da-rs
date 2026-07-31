/**
 * TagBrowserModal — left branch tree + right leaf list, multi-select.
 *
 * Backed by the lazy `browse_children` IPC:
 * - Clicking a branch *row* selects it AND loads its leaves (right pane).
 * - Clicking ▸ toggles subtree expansion (loads child branches).
 * Opening the modal loads root's children first.
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
    const [loadingKey, setLoadingKey] = useState<string | null>(null);

    // Load root children on open.
    useEffect(() => {
        setError(null);
        setLoadingKey(ROOT_KEY);
        browseChildren(null)
            .then((c) => {
                setRoots(c.branches);
                setLeavesByBranch((m) => new Map(m).set(ROOT_KEY, c.leaves));
            })
            .catch((e) => setError(String(e)))
            .finally(() => setLoadingKey(null));
    }, [progId]);

    // Ensure a branch's children + leaves are loaded (idempotent on the cache).
    const ensureLoaded = useCallback(
        async (branchId: string) => {
            if (leavesByBranch.has(branchId)) return;
            setLoadingKey(branchId);
            try {
                const c = await browseChildren(branchId);
                setChildBranches((m) => new Map(m).set(branchId, c.branches));
                setLeavesByBranch((m) => new Map(m).set(branchId, c.leaves));
            } catch (e) {
                setError(String(e));
            } finally {
                setLoadingKey(null);
            }
        },
        [leavesByBranch],
    );

    // Click a branch row: select it + load its leaves into the right pane.
    const pickBranch = (id: string | null) => {
        setSelectedBranch(id);
        if (id !== null) void ensureLoaded(id);
    };

    // Click ▸: toggle subtree expansion (ensures child branches loaded).
    const toggleExpand = (branch: BranchNode) => {
        void ensureLoaded(branch.id);
        setExpanded((s) => {
            const next = new Set(s);
            if (next.has(branch.id)) next.delete(branch.id);
            else next.add(branch.id);
            return next;
        });
    };

    const currentKey = branchKey(selectedBranch);
    const currentLeaves = leavesByBranch.get(currentKey) ?? [];
    const isLoading = loadingKey === currentKey;
    const visibleLeaves = useMemo(() => {
        const f = leafFilter.trim().toLowerCase();
        if (!f) return currentLeaves;
        return currentLeaves.filter(
            (l) => l.name.toLowerCase().includes(f) || l.item_id.toLowerCase().includes(f),
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
                    onClick={() => pickBranch(branch.id)}
                >
                    <span
                        className="tree-twisty"
                        onClick={(e) => {
                            e.stopPropagation();
                            toggleExpand(branch);
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
                            onClick={() => pickBranch(null)}
                        >
                            <span className="tree-twisty"> </span>
                            🌐 {progId ?? "(root)"}
                        </div>
                        {roots.map((b) => renderBranch(b, 1))}
                    </div>
                    <div className="leaf-pane">
                        <div className="leaf-toolbar">
                            <input
                                type="text"
                                placeholder="filter leaves…"
                                value={leafFilter}
                                onChange={(e) => setLeafFilter(e.target.value)}
                                style={{ flex: 1, padding: "2px 6px" }}
                            />
                            <button
                                className="mini"
                                onClick={() =>
                                    setSelectedLeaves(
                                        (s) =>
                                            new Set([
                                                ...s,
                                                ...visibleLeaves.map((l) => l.item_id),
                                            ]),
                                    )
                                }
                                disabled={visibleLeaves.length === 0}
                                title="Select all visible leaves"
                            >
                                All
                            </button>
                            <button
                                className="mini"
                                onClick={() => setSelectedLeaves(new Set())}
                                disabled={selectedLeaves.size === 0}
                                title="Clear selection"
                            >
                                None
                            </button>
                        </div>
                        <div className="leaf-list">
                            {isLoading ? (
                                <div style={{ padding: 8, color: "#858585", fontSize: 11 }}>
                                    Loading…
                                </div>
                            ) : visibleLeaves.length === 0 ? (
                                <div style={{ padding: 8, color: "#858585", fontSize: 11 }}>
                                    No tags under this node.
                                </div>
                            ) : (
                                visibleLeaves.map((l) => (
                                    <div
                                        key={l.item_id}
                                        className={`leaf ${selectedLeaves.has(l.item_id) ? "selected" : ""}`}
                                        onClick={() => toggleLeaf(l.item_id)}
                                        title={l.item_id}
                                    >
                                        {selectedLeaves.has(l.item_id) ? "☑" : "☐"} {l.name}
                                    </div>
                                ))
                            )}
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
