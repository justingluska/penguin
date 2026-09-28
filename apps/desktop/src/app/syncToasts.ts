// Sync problems as toasts: when an account's sync breaks, one error toast
// says so with the fix button (features/settings/syncFix): "Retry sync" for
// errors, "Reconnect" for a sign-in that has to be redone. The same error on
// several accounts is one toast ("Can't reach Gmail right now · 3 accounts").
// A toast leaves on its own when the problem is resolved. The status bar and
// the sidebar sync popover keep showing the state; this only announces it.
import { dismissToastKey, toast } from "../components/Toast";
import { accountName } from "../components/Identity";
import { runSyncFix, syncFixFor, type SyncFix } from "../features/settings/syncFix";
import { meta } from "./store";

interface Issue {
  key: string;
  fix: SyncFix;
  accountIds: string[];
}

function currentIssues(): Map<string, Issue> {
  const { accounts, sync } = meta.get();
  const issues = new Map<string, Issue>();
  for (const a of accounts) {
    const fix = syncFixFor(sync[a.id]);
    if (!fix) continue;
    // Reconnect is per account (each signs in on its own); retries group by message.
    const key = fix.kind === "reconnect" ? `sync:reconnect:${a.id}` : `sync:retry:${fix.message}`;
    const cur = issues.get(key);
    if (cur) cur.accountIds.push(a.id);
    else issues.set(key, { key, fix, accountIds: [a.id] });
  }
  return issues;
}

function show(issue: Issue) {
  const { accounts } = meta.get();
  const names = issue.accountIds.map((id) => {
    const a = accounts.find((x) => x.id === id);
    return a ? accountName(a, accounts) : id;
  });
  const n = names.length;
  const { fix } = issue;
  toast({
    kind: "error",
    key: issue.key,
    message:
      fix.kind === "reconnect"
        ? `${names[0]}: ${fix.message.charAt(0).toLowerCase()}${fix.message.slice(1)}`
        : n > 1
          ? `${fix.message} · ${n} accounts`
          : fix.message,
    detail: fix.kind === "retry" ? names.join(", ") : undefined,
    title: fix.detail ?? undefined,
    actions: [
      {
        label: fix.label,
        run: () => Promise.all(issue.accountIds.map((id) => runSyncFix(fix, id))),
      },
    ],
  });
}

const signature = (i: Issue) => i.accountIds.join(",");

/** Toast every current sync problem now (e.g. at the end of a refresh), even ones dismissed before. */
export function showSyncIssues() {
  for (const issue of currentIssues().values()) show(issue);
}

/** Announce sync problems as they appear and clear them when resolved. Returns the unsubscribe. */
export function startSyncToasts(): () => void {
  let shown = new Map<string, string>(); // key -> signature last announced
  const check = () => {
    const issues = currentIssues();
    for (const [key, issue] of issues) {
      // New problem, or another account joined it: announce (again).
      if (shown.get(key) !== signature(issue)) show(issue);
    }
    for (const key of shown.keys()) if (!issues.has(key)) dismissToastKey(key);
    shown = new Map([...issues].map(([k, i]) => [k, signature(i)]));
  };
  let last = meta.get().sync;
  let lastAccounts = meta.get().accounts;
  check();
  return meta.subscribe(() => {
    const m = meta.get();
    if (m.sync === last && m.accounts === lastAccounts) return;
    last = m.sync;
    lastAccounts = m.accounts;
    check();
  });
}
