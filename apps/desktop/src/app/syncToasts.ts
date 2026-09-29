// Sync problems as toasts. An account's sync problem is announced only once
// it's worth it (lib/syncHealth.ts): it needs the user, or the engine's own
// retries have failed for a while (3 attempts or a minute). A one-off failure
// the next attempt fixes never shows. The same problem on several accounts is
// one toast ("Can't reach Gmail right now · 3 accounts").
//
// The toast carries the actions (features/settings/syncFix): Retry now (or
// Reconnect), Copy details, Hide for 6 hours, Open logs. While a retry runs
// it says so; when the problem clears by itself or by a retry, it turns into
// "Syncing again" instead of vanishing. Hidden alerts (per device, per
// account and error kind) don't toast until the hide runs out or the kind
// changes. The status bar and the sidebar sync popover keep showing the state.
import { dismissToastKey, toast, type ToastAction } from "../components/Toast";
import { accountName } from "../components/Identity";
import {
  canHide,
  copySyncDetails,
  hideSyncAlerts,
  isRetrying,
  retryNow,
  retryOutcome,
  runSyncFix,
  subscribeRetries,
  syncFixFor,
  type SyncFix,
} from "../features/settings/syncFix";
import { openLogViewer } from "../features/logs/LogViewer";
import { activeHide } from "../lib/syncHealth";
import { getSyncHides, subscribeSyncHides } from "../lib/syncHides";
import { meta } from "./store";

interface Issue {
  key: string;
  fix: SyncFix;
  accountIds: string[];
}

function currentIssues(): Map<string, Issue> {
  const { accounts, sync } = meta.get();
  const hides = getSyncHides();
  const issues = new Map<string, Issue>();
  for (const a of accounts) {
    const s = sync[a.id];
    const fix = syncFixFor(s);
    if (!fix || activeHide(hides, s)) continue;
    // Reconnect is per account (each signs in on its own); retries group by message.
    const key = fix.kind === "reconnect" ? `sync:reconnect:${a.id}` : `sync:retry:${fix.message}`;
    const cur = issues.get(key);
    if (cur) cur.accountIds.push(a.id);
    else issues.set(key, { key, fix, accountIds: [a.id] });
  }
  return issues;
}

const names = (ids: string[]) => {
  const { accounts } = meta.get();
  return ids.map((id) => {
    const a = accounts.find((x) => x.id === id);
    return a ? accountName(a, accounts) : id;
  });
};

function show(issue: Issue) {
  const who = names(issue.accountIds);
  const n = who.length;
  const { fix } = issue;
  if (fix.kind === "retry" && issue.accountIds.some(isRetrying)) {
    toast({ kind: "progress", key: issue.key, message: `Retrying ${who.join(", ")}…`, detail: fix.message, actions: [], progress: null });
    return;
  }
  const { sync } = meta.get();
  // A retry of this streak failed (not one from an earlier, recovered streak).
  const failedAgain =
    fix.kind === "retry" &&
    issue.accountIds.some((id) => {
      const r = retryOutcome(id);
      return r?.outcome === "failed" && r.at >= (sync[id]?.failure?.firstAt ?? Infinity);
    });
  const attempts = Math.max(...issue.accountIds.map((id) => sync[id]?.failure?.count ?? 0));
  const actions: ToastAction[] = [
    {
      label: fix.label,
      keep: fix.kind === "retry",
      run: () => Promise.all(issue.accountIds.map((id) => (fix.kind === "retry" ? retryNow(id) : runSyncFix(fix, id)))),
    },
    { label: "Copy details", keep: true, run: () => copySyncDetails(issue.accountIds) },
  ];
  if (issue.accountIds.every((id) => canHide(sync[id]))) actions.push({ label: "Hide 6 h", run: () => hideSyncAlerts(issue.accountIds) });
  actions.push({ label: "Open logs", keep: true, run: openLogViewer });
  const base = fix.kind === "reconnect" ? `${who[0]}: ${fix.message.charAt(0).toLowerCase()}${fix.message.slice(1)}` : n > 1 ? `${fix.message} · ${n} accounts` : fix.message;
  toast({
    kind: "error",
    key: issue.key,
    message: failedAgain ? `Still failing: ${base.charAt(0).toLowerCase()}${base.slice(1)}` : base,
    detail:
      fix.kind === "retry"
        ? [who.join(", "), attempts > 1 ? `${attempts} tries so far; retrying on its own` : null].filter(Boolean).join(" · ")
        : undefined,
    title: fix.detail ?? undefined,
    progress: undefined,
    actions,
  });
}

/** The problem went away: say so in its toast rather than letting it vanish. */
function resolved(key: string, accountIds: string[]) {
  const { accounts, sync } = meta.get();
  const recovered = accountIds.filter((id) => accounts.some((a) => a.id === id) && sync[id]?.recovered && !sync[id]?.failure);
  if (recovered.length === 0) return dismissToastKey(key);
  const who = names(recovered);
  toast({
    kind: "success",
    key,
    message: `${who.length > 1 ? `${who.length} accounts are` : `${who[0]} is`} syncing again`,
    detail: who.length > 1 ? who.join(", ") : undefined,
    title: undefined,
    progress: undefined,
    actions: [],
    duration: 4_000,
  });
}

// The last announced state per key, so an unchanged issue isn't re-toasted.
const signature = (i: Issue) =>
  [
    i.accountIds.join(","),
    i.accountIds.some(isRetrying) ? "retrying" : "",
    i.accountIds.map((id) => retryOutcome(id)?.at ?? 0).join(","),
    i.accountIds.every((id) => canHide(meta.get().sync[id])) ? "h" : "",
  ].join("|");

/** Toast every current sync problem now (e.g. at the end of a refresh), even ones dismissed before. */
export function showSyncIssues() {
  for (const issue of currentIssues().values()) show(issue);
}

/** Announce sync problems as they become worth it and settle them when resolved. Returns the unsubscribe. */
export function startSyncToasts(): () => void {
  let shown = new Map<string, { sig: string; accountIds: string[] }>();
  const check = () => {
    const issues = currentIssues();
    for (const [key, issue] of issues) {
      // New problem, another account joined it, or a retry started or ended: announce (again).
      if (shown.get(key)?.sig !== signature(issue)) show(issue);
    }
    const hidden = new Set(getSyncHides().map((h) => h.accountId));
    for (const [key, was] of shown) {
      if (issues.has(key)) continue;
      // Hidden by the user: just go. Otherwise it went away: say how.
      if (was.accountIds.every((id) => hidden.has(id))) dismissToastKey(key);
      else resolved(key, was.accountIds);
    }
    shown = new Map([...issues].map(([k, i]) => [k, { sig: signature(i), accountIds: i.accountIds }]));
  };
  let last = meta.get().sync;
  let lastAccounts = meta.get().accounts;
  check();
  const offMeta = meta.subscribe(() => {
    const m = meta.get();
    if (m.sync === last && m.accounts === lastAccounts) return;
    last = m.sync;
    lastAccounts = m.accounts;
    check();
  });
  const offHides = subscribeSyncHides(check);
  const offRetries = subscribeRetries(check);
  return () => {
    offMeta();
    offHides();
    offRetries();
  };
}
