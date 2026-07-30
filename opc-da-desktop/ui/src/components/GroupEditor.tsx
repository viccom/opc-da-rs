/**
 * GroupEditor — main-pane top strip for the active group.
 *
 * Edit the selected group's name + refresh rate, add tags via the modal
 * browser (merged into the group's tag set, de-duplicated), and
 * start/stop the group's subscription.
 */

import { useState } from "react";
import { useSubscriptionStore } from "../stores/subscription";
import { TagBrowserModal } from "./TagBrowserModal";

export function GroupEditor() {
    const group = useSubscriptionStore((s) =>
        s.activeGroupId ? s.groups.get(s.activeGroupId) : undefined,
    );
    const setGroupName = useSubscriptionStore((s) => s.setGroupName);
    const setGroupRate = useSubscriptionStore((s) => s.setGroupRate);
    const setGroupTags = useSubscriptionStore((s) => s.setGroupTags);
    const startGroup = useSubscriptionStore((s) => s.startGroup);
    const stopGroup = useSubscriptionStore((s) => s.stopGroup);

    const [showModal, setShowModal] = useState(false);

    if (!group) return null;

    const onSubscribe = async () => {
        if (group.tagIds.length === 0) return;
        await startGroup(group.id);
    };
    const onUnsubscribe = async () => {
        await stopGroup(group.id);
    };

    return (
        <div className="panel group-editor">
            <h3>3. Subscription Group</h3>
            <div className="field">
                <label>Name</label>
                <input
                    value={group.name}
                    onChange={(e) => setGroupName(group.id, e.target.value)}
                />
            </div>
            <div className="field">
                <label>Rate (ms)</label>
                <input
                    type="number"
                    value={group.rate}
                    onChange={(e) => setGroupRate(group.id, Number(e.target.value))}
                />
            </div>
            {group.error && (
                <div className="field" style={{ color: "#f48771" }}>
                    {group.error}
                </div>
            )}
            <div className="field">
                <button onClick={() => setShowModal(true)}>Add Tags…</button>
                <button
                    onClick={onSubscribe}
                    disabled={group.busy || group.tagIds.length === 0 || group.cookie !== null}
                >
                    Start ({group.tagIds.length})
                </button>
                {group.cookie !== null && (
                    <button className="danger" onClick={onUnsubscribe} disabled={group.busy}>
                        Stop
                    </button>
                )}
            </div>
            {showModal && (
                <TagBrowserModal
                    onAdd={(ids) => {
                        const merged = Array.from(new Set([...group.tagIds, ...ids]));
                        setGroupTags(group.id, merged);
                        setShowModal(false);
                    }}
                    onClose={() => setShowModal(false)}
                />
            )}
        </div>
    );
}
