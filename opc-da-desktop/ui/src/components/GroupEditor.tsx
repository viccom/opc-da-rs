/**
 * GroupEditor — top-right strip (function 4).
 *
 * Lets the user configure subscription group parameters and add tags
 * via the modal browser.
 */

import { useState } from "react";
import { useSubscriptionStore } from "../stores/subscription";
import { TagBrowserModal } from "./TagBrowserModal";

export function GroupEditor() {
  const [name, setName] = useState("Group1");
  const [rate, setRate] = useState(1000);
  const [deadband, setDeadband] = useState(0);
  const [clientHandle, setClientHandle] = useState(1);
  const [showModal, setShowModal] = useState(false);
  const [pendingTags, setPendingTags] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { cookie, start, stop } = useSubscriptionStore();

  const onAddTags = (ids: string[]) => {
    setPendingTags(ids);
    setShowModal(false);
  };

  const onSubscribe = async () => {
    if (pendingTags.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      await start(pendingTags, rate);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onUnsubscribe = async () => {
    setBusy(true);
    try {
      await stop();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="panel">
      <h3>3 &amp; 4. Subscription Group</h3>
      <div className="field">
        <label>Name</label>
        <input value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label>Rate (ms)</label>
        <input
          type="number"
          value={rate}
          onChange={(e) => setRate(Number(e.target.value))}
        />
      </div>
      <div className="field">
        <label>Deadband %</label>
        <input
          type="number"
          value={deadband}
          onChange={(e) => setDeadband(Number(e.target.value))}
        />
      </div>
      <div className="field">
        <label>Client Handle</label>
        <input
          type="number"
          value={clientHandle}
          onChange={(e) => setClientHandle(Number(e.target.value))}
        />
      </div>
      {error && (
        <div className="field" style={{ color: "#f48771" }}>
          {error}
        </div>
      )}
      <div className="field">
        <button onClick={() => setShowModal(true)} disabled={!name}>
          Add Tags…
        </button>
        <button
          onClick={onSubscribe}
          disabled={busy || pendingTags.length === 0 || cookie !== null}
        >
          Start ({pendingTags.length})
        </button>
        {cookie !== null && (
          <button className="danger" onClick={onUnsubscribe} disabled={busy}>
            Stop
          </button>
        )}
      </div>
      {showModal && <TagBrowserModal onAdd={onAddTags} onClose={() => setShowModal(false)} />}
    </div>
  );
}