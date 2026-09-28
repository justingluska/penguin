// Sidebar sync block: combined backfill progress with an ETA (or a note that
// an account needs attention). Clicking it opens a per-account breakdown.
import { useEffect, useRef, useSyncExternalStore } from "react";
import type { Account, SyncPhase, SyncStatus } from "../../lib/types";
import { ago, eta, num } from "../../lib/format";
import { AccountDot, accountName } from "../../components/Identity";
import { meta } from "../../app/store";
import { SyncFixButton, syncFixFor } from "../settings/syncFix";
import { getLayout, setLayout } from "../../lib/layout";
import { useDismiss } from "../../lib/dismiss";

const PHASE: Record<SyncPhase, string> = {
  backfilling: "Backfilling",
  incremental: "Syncing",
  idle: "Up to date",
  error: "Error",
  needsReauth: "Signed out",
};

const needsAttention = (s: SyncStatus) => s.phase === "error" || s.phase === "needsReauth";
const pct = (indexed: number, total: number | null) => (total ? Math.min(100, (indexed / total) * 100) : 0);
/**
 * Indexed as shown against the estimate. The estimate excludes spam/trash, so
 * mail that lands there after syncing can push indexed slightly past it.
 */
const shownIndexed = (s: SyncStatus) => (s.totalEstimate === null ? s.indexed : Math.min(s.indexed, s.totalEstimate));

// Open state lives outside React so the status bar can open the popover too.
let popoverOpen = false;
const popoverSubs = new Set<() => void>();
function setPopoverOpen(open: boolean) {
  if (open === popoverOpen) return;
  popoverOpen = open;
  popoverSubs.forEach((f) => f());
}

/**
 * The status bar's sync line: open the per-account breakdown (showing the
 * sidebar if hidden), or close it when it's already open. The button carries
 * SYNC_TOGGLE so its press doesn't count as a click away.
 */
export function toggleSyncPopover() {
  if (popoverOpen) return setPopoverOpen(false);
  if (getLayout().sidebarCollapsed) setLayout({ sidebarCollapsed: false });
  setPopoverOpen(true);
}
export const SYNC_TOGGLE = "[data-sync-toggle]";

/**
 * Accounts backfill in parallel, so the combined ETA is the slowest account's.
 * Unknown while any backfilling account is still estimating.
 */
function combinedEta(backfilling: SyncStatus[]): number | null {
  let max = 0;
  for (const s of backfilling) {
    if (s.etaSecs === null) return null;
    max = Math.max(max, s.etaSecs);
  }
  return backfilling.length ? max : null;
}

/**
 * `accounts` is the scope (the active profile's accounts, or all). Progress
 * narrows to it; an account that needs attention is shown wherever it is,
 * since it can't sync until someone acts.
 */
export function SyncProgress({ accounts: scoped }: { accounts: Account[] }) {
  const sync = meta.use((m) => m.sync);
  const allAccounts = meta.use((m) => m.accounts);
  const open = useSyncExternalStore(
    (cb) => {
      popoverSubs.add(cb);
      return () => popoverSubs.delete(cb);
    },
    () => popoverOpen,
  );
  const setOpen = (o: boolean) => setPopoverOpen(o);
  const ref = useRef<HTMLDivElement>(null);
  const all = Object.values(sync);
  const inScope = new Set(scoped.map((a) => a.id));
  const backfilling = all.filter((s) => s.phase === "backfilling" && inScope.has(s.accountId));
  const attention = all.filter(needsAttention);
  const accounts = allAccounts.filter((a) => inScope.has(a.id) || (sync[a.id] && needsAttention(sync[a.id])));
  // Settled and healthy: nothing to show (the status bar carries "Synced …"),
  // unless the status bar opened the breakdown.
  const show = backfilling.length > 0 || attention.length > 0 || open;
  // Unmounting (sidebar hidden, Settings open) closes it.
  useEffect(() => () => setPopoverOpen(false), []);

  useDismiss(open, () => setOpen(false), [ref], SYNC_TOGGLE);
  useEffect(() => {
    if (!open) return;
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setOpen(false);
      }
    };
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  if (!show) return null;

  let indexed = 0;
  let total = 0;
  for (const s of backfilling) {
    indexed += shownIndexed(s);
    total += s.totalEstimate ?? s.indexed;
  }
  const left = combinedEta(backfilling);
  const leftText = left === null ? "estimating…" : `${eta(left)} left`;

  return (
    <div className="sync-wrap" ref={ref}>
      <button
        className={"sb-progress" + (open ? " is-open" : "")}
        aria-expanded={open}
        aria-haspopup="dialog"
        title="Sync details"
        onClick={() => setOpen(!open)}
      >
        {backfilling.length > 0 ? (
          <>
            <span className="sb-progress-row">
              <span className="row-flex" style={{ gap: 6 }}>
                {attention.length > 0 && <span className="sync-dot is-error" title="An account needs attention" />}
                Backfilling
              </span>
              <span className="tnum">{leftText}</span>
            </span>
            <span className="sb-bar">
              <span style={{ width: `${pct(indexed, total)}%` }} />
            </span>
            <span className="sb-progress-sub tnum">
              {num(indexed)} of {num(total)}
            </span>
          </>
        ) : attention.length === 0 ? (
          <span className="sb-progress-row is-alert">
            <span className="sync-dot" />
            <span className="grow truncate">All accounts up to date</span>
          </span>
        ) : (
          <span className="sb-progress-row is-alert">
            <span className="sync-dot is-error" />
            <span className="grow truncate">
              {attention.length === 1 ? "1 account needs attention" : `${attention.length} accounts need attention`}
            </span>
          </span>
        )}
      </button>
      {open && <SyncPopover accounts={accounts} sync={sync} />}
    </div>
  );
}

function SyncPopover({ accounts, sync }: { accounts: Account[]; sync: Record<string, SyncStatus> }) {
  return (
    <div className="panel sync-pop" role="dialog" aria-label="Sync status">
      {accounts.map((a) => {
        const s = sync[a.id];
        return (
          <div key={a.id} className="sync-acct">
            <div className="sync-acct-head">
              <AccountDot color={a.color} size="sm" />
              <span className="grow truncate" title={accountName(a, accounts)}>
                {a.email}
              </span>
              <span className={"sync-acct-phase" + (s && needsAttention(s) ? " is-alert" : "")}>{s ? PHASE[s.phase] : "Waiting"}</span>
            </div>
            {s && <AccountSyncDetail s={s} />}
          </div>
        );
      })}
    </div>
  );
}

function AccountSyncDetail({ s }: { s: SyncStatus }) {
  if (s.phase === "backfilling") {
    const parts = [
      `${num(shownIndexed(s))} of ${s.totalEstimate === null ? "…" : num(s.totalEstimate)}`,
      s.ratePerMin === null ? null : `~${num(Math.round(s.ratePerMin))}/min`,
      s.etaSecs === null ? "estimating…" : `${eta(s.etaSecs)} left`,
    ].filter(Boolean);
    return (
      <>
        <span className="sb-bar sync-acct-bar">
          <span style={{ width: `${pct(s.indexed, s.totalEstimate)}%` }} />
        </span>
        <div className="sync-acct-line tnum">{parts.join(" · ")}</div>
      </>
    );
  }
  const fix = syncFixFor(s);
  if (fix) {
    return (
      <div className="sync-acct-fix">
        <span className="sync-acct-line is-alert grow" title={fix.detail ?? undefined}>
          {fix.message}
        </span>
        <SyncFixButton status={s} />
      </div>
    );
  }
  return (
    <div className="sync-acct-line tnum">
      {num(s.indexed)} messages{s.lastSyncedAt ? ` · synced ${ago(s.lastSyncedAt)}` : ""}
    </div>
  );
}
