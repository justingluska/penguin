// Settings → Search → "Search by meaning": one switch plus a quiet progress
// line (model download, indexing, paused, caught up). Polls semantic_status
// while the page is open; the status is local and instant.
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import type { SemanticIndexStatus } from "../../lib/types";
import { Switch } from "./parts";
import { semanticLine } from "./semanticLine";

const POLL_MS = 2000;

export function SemanticSearchRow() {
  const on = useSetting("semanticSearch");
  const [status, setStatus] = useState<SemanticIndexStatus | null>(null);
  useEffect(() => {
    let alive = true;
    const tick = () =>
      api
        .semanticStatus()
        .then((s) => alive && setStatus(s))
        .catch(() => alive && setStatus(null));
    void tick();
    const id = window.setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [on]);
  return (
    <div className="setting-row setting-tall" data-setting="semantic-search">
      <div>
        <span className="setting-label">Search by meaning</span>
        <p className="st-muted">
          Also finds mail that says the same thing in other words or another language. Runs on this Mac: the model is downloaded once and
          no mail leaves your computer.
          {status ? ` Model: ${status.model}, provided under the ${status.modelLicense}.` : ""}
        </p>
        <p className="st-muted semantic-progress tnum" aria-live="polite">
          {semanticLine(status)}
        </p>
      </div>
      <Switch label="Search by meaning" on={on} onChange={(semanticSearch) => void updateSettings({ semanticSearch })} />
    </div>
  );
}
