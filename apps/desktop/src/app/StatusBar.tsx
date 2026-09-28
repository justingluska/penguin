// Bottom hint bar: the keys for the current context, and sync status.
import { useEffect, useState } from "react";
import { useUi } from "../lib/ui";
import { subscribePendingSequence } from "../lib/keyboard";
import { ago, num } from "../lib/format";
import { Keys } from "../components/Kbd";
import { meta } from "./store";
import { accountName } from "../components/Identity";
import { SyncFixButton, syncFixFor } from "../features/settings/syncFix";
import { toggleSyncPopover } from "../features/sidebar/SyncProgress";
import { NextUp } from "../features/calendar/NextUp";

const LIST_HINTS: [string, string][] = [
  ["j k", "Move"],
  ["e", "Done"],
  ["r", "Reply"],
  ["/", "Search"],
  ["mod+k", "Commands"],
  ["?", "All shortcuts"],
];
const THREAD_HINTS: [string, string][] = [
  ["n p", "Next / prev message"],
  ["r", "Reply"],
  ["a", "Reply all"],
  ["f", "Forward"],
  ["e", "Done"],
  ["o", "Expand all"],
  ["i", "Toggle panel"],
];

const CALENDAR_HINTS: [string, string][] = [
  ["j k", "Move"],
  ["enter", "Open"],
  ["a", "Agenda"],
  ["w", "Week"],
  ["m", "Month"],
  ["t", "Today"],
  ["g i", "Inbox"],
];

export function StatusBar() {
  const threadOpen = useUi((s) => s.threadOpen);
  const calendar = useUi((s) => s.surface === "calendar");
  const [pending, setPending] = useState<string | null>(null);
  useEffect(() => subscribePendingSequence(setPending), []);
  const hints = threadOpen ? THREAD_HINTS : calendar ? CALENDAR_HINTS : LIST_HINTS;
  return (
    <footer className="statusbar">
      {pending ? (
        <span className="hint">
          <Keys keys={pending} />
          <span className="faint">then… i Inbox · s Starred · t Sent · d Drafts · e Done · a All mail · c Calendar</span>
        </span>
      ) : (
        hints.map(([k, label]) => (
          <span key={k} className="hint">
            {k.includes(" ") ? (
              <span className="kbd-group">
                {k.split(" ").map((c) => (
                  <Keys key={c} keys={c} />
                ))}
              </span>
            ) : (
              <Keys keys={k} />
            )}
            {label}
          </span>
        ))
      )}
      <NextUp />
      <SyncStatusLine />
    </footer>
  );
}

function SyncStatusLine() {
  const sync = meta.use((m) => m.sync);
  const accounts = meta.use((m) => m.accounts);
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 15_000);
    return () => clearInterval(t);
  }, []);

  const all = Object.values(sync);
  // Accounts that need a fix; reconnects first (sync can't resume without one).
  const broken = all.find((s) => syncFixFor(s)?.kind === "reconnect") ?? all.find((s) => syncFixFor(s));
  const fix = syncFixFor(broken);
  const backfilling = all.filter((s) => s.phase === "backfilling");
  const nameOf = (id: string) => {
    const a = accounts.find((x) => x.id === id);
    return a ? accountName(a, accounts) : id;
  };

  let dot = "sync-dot";
  let text: string;
  if (broken && fix) {
    dot += " is-error";
    text = `${nameOf(broken.accountId)} · ${fix.message}`;
  } else if (backfilling.length) {
    dot += " is-busy";
    const indexed = backfilling.reduce((n, s) => n + s.indexed, 0);
    const total = backfilling.reduce((n, s) => n + (s.totalEstimate ?? s.indexed), 0);
    text = `Backfilling ${num(indexed)} of ${num(total)}`;
  } else {
    const times = all.map((s) => s.lastSyncedAt ?? 0).filter(Boolean);
    const oldest = times.length ? Math.min(...times) : null;
    const n = accounts.length;
    text = `${oldest ? `Synced ${ago(oldest)}` : "Not synced yet"} · ${n} ${n === 1 ? "account" : "accounts"}`;
  }
  const title = all.map((s) => `${nameOf(s.accountId)}: ${s.phase}, ${num(s.indexed)} indexed`).join("\n");
  if (broken && fix) {
    return (
      <span className="right sync-broken">
        <button className="sync-status-btn" title={fix.detail ?? title} data-sync-toggle onClick={toggleSyncPopover}>
          <span className={dot} />
          {text}
        </button>
        <SyncFixButton status={broken} className="btn btn-secondary btn-sm sync-fix" />
      </span>
    );
  }
  return (
    <button className="right sync-status-btn" title={title} data-sync-toggle onClick={toggleSyncPopover}>
      <span className={dot} />
      {text}
    </button>
  );
}
