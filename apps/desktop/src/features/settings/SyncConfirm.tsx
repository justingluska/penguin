// The confirmation behind Settings → Sync: moving the window slider or
// changing "Older mail" stages a pending value, and nothing is saved until
// this dialog is confirmed. It spells out what the change costs (messages,
// time at the account's quota, disk) with a per-account breakdown.
// OWNER: sync-window agent.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import type { Account, FreeUpSpace, OlderMail, SyncCoverage, SyncWindowMonths, WindowEstimate } from "../../lib/types";
import { bytes, eta, num } from "../../lib/format";
import { accountName } from "../../components/Identity";

export type PendingChange = { kind: "window"; months: SyncWindowMonths } | { kind: "older"; mode: OlderMail };

/** Slider order: shortest first, "everything" (0) last. */
export function windowRank(m: SyncWindowMonths): number {
  return m === 0 ? Number.MAX_SAFE_INTEGER : m;
}

export function windowLabel(m: SyncWindowMonths): string {
  if (m === 0) return "Everything";
  if (m === 1) return "1 month";
  if (m === 12) return "1 year";
  if (m === 24) return "2 years";
  return `${m} months`;
}

/** "the last 6 months" / "all mail", for sentences. */
export function windowPhrase(m: SyncWindowMonths): string {
  if (m === 0) return "all mail";
  if (m === 1) return "the last month";
  if (m === 12) return "the last year";
  return `the last ${windowLabel(m).toLowerCase()}`;
}

const accountsPhrase = (n: number) => (n === 1 ? "1 account" : `${n} accounts`);

/** One row of the per-account breakdown. */
interface Line {
  accountId: string;
  messages: number;
  secs: number;
  note?: string;
}

interface Plan {
  title: string;
  summary: ReactNode;
  lines: Line[];
  confirm: string;
}

/**
 * What a pending change means, from cached estimates. Null while the
 * numbers it needs are still loading.
 */
export function planFor(
  change: PendingChange,
  current: { months: SyncWindowMonths; older: OlderMail },
  estimates: (m: SyncWindowMonths) => WindowEstimate[] | null,
  coverage: SyncCoverage | null,
  shrinkPreview: FreeUpSpace | null,
): Plan | null {
  if (change.kind === "window") {
    const target = change.months;
    const grow = windowRank(target) > windowRank(current.months);
    if (grow) {
      const rows = estimates(target);
      if (!rows) return null;
      const ok = rows.filter((r) => !r.error);
      const lines: Line[] = ok.map((r) => ({ accountId: r.accountId, messages: Math.max(0, r.inWindow - r.haveFull), secs: r.etaSecs }));
      const more = lines.reduce((n, l) => n + l.messages, 0);
      const secs = Math.max(0, ...lines.map((l) => l.secs));
      const perMsg = ok[0]?.bytesPerMessage ?? 0;
      return {
        title: `Download ${windowPhrase(target)} in full?`,
        summary:
          more === 0 ? (
            <>Everything in {windowPhrase(target)} is already on this Mac.</>
          ) : (
            <>
              Download full text for ~{num(more)} more messages across {accountsPhrase(lines.filter((l) => l.messages > 0).length)} · {eta(secs)} at your
              quota · uses about {bytes(more * perMsg)}.
            </>
          ),
        lines,
        confirm: "Download",
      };
    }
    if (!shrinkPreview) return null;
    return {
      title: `Keep ${windowPhrase(target)} in full?`,
      summary: (
        <>
          New mail and messages from {windowPhrase(target)} stay fully downloaded. The {num(shrinkPreview.messages)} older full messages stay on this
          Mac. Nothing is deleted.
        </>
      ),
      lines: [],
      confirm: "Save",
    };
  }

  // Older mail.
  const rows = estimates(current.months);
  if (!rows || !coverage) return null;
  const ok = rows.filter((r) => !r.error);
  const cov = (id: string) => coverage.accounts.find((a) => a.accountId === id);
  if (change.mode === "none") {
    const kept = ok.reduce((n, r) => n + (cov(r.accountId)?.headersOnly ?? 0), 0);
    return {
      title: "Stop downloading older mail?",
      summary: (
        <>
          Mail older than {windowLabel(current.months).toLowerCase()} won't be downloaded any more. The {num(kept)} older messages already on this
          Mac stay, and “Also search Gmail” still finds the rest.
        </>
      ),
      lines: [],
      confirm: "Save",
    };
  }
  const full = change.mode === "full";
  const lines: Line[] = ok.map((r) => {
    const c = cov(r.accountId);
    // Older mail already stored: everything local minus what's in the window.
    const olderFull = Math.max(0, (c?.full ?? 0) - r.haveFull);
    const have = full ? olderFull : olderFull + (c?.headersOnly ?? 0);
    const messages = Math.max(0, r.older - have);
    const perAll = full ? r.olderFullEtaSecs : r.olderHeadersEtaSecs;
    return { accountId: r.accountId, messages, secs: r.older > 0 ? Math.round((perAll * messages) / r.older) : 0 };
  });
  const total = lines.reduce((n, l) => n + l.messages, 0);
  const secs = Math.max(0, ...lines.map((l) => l.secs));
  const perMsg = ok[0]?.bytesPerMessage ?? 0;
  const busy = accountsPhrase(lines.filter((l) => l.messages > 0).length);
  return {
    title: full ? "Download older mail in full?" : "Fetch headers for older mail?",
    summary:
      total === 0 ? (
        <>Older mail is already on this Mac.</>
      ) : full ? (
        <>
          Download full text for ~{num(total)} older messages across {busy} · {eta(secs)} at your quota, after the window finishes · uses about{" "}
          {bytes(total * perMsg)}.
        </>
      ) : (
        <>
          Fetch sender, subject, labels and previews for ~{num(total)} older messages across {busy} · {eta(secs)} at your quota, after the window
          finishes. Bodies download when you open a message.
        </>
      ),
    lines,
    confirm: full ? "Download" : "Fetch headers",
  };
}

export function SyncConfirm({
  change,
  current,
  estimates,
  coverage,
  accounts,
  onConfirm,
  onCancel,
}: {
  change: PendingChange;
  current: { months: SyncWindowMonths; older: OlderMail };
  estimates: (m: SyncWindowMonths) => WindowEstimate[] | null;
  coverage: SyncCoverage | null;
  accounts: Account[];
  /** `freeUp`: also drop bodies older than the new (smaller) window. */
  onConfirm: (freeUp: boolean) => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const [freeUp, setFreeUp] = useState(false);
  const shrink = change.kind === "window" && windowRank(change.months) < windowRank(current.months);
  const [preview, setPreview] = useState<FreeUpSpace | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  useEffect(() => {
    if (!shrink || change.kind !== "window") return;
    let live = true;
    api.freeUpSpace(true, null, change.months).then(
      (p) => live && setPreview(p),
      (e) => live && setPreviewError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, [shrink, change]);

  useEffect(() => {
    cancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  const plan = planFor(change, current, estimates, coverage, preview ?? (previewError ? { messages: 0, bodyBytes: 0, bytesBefore: 0, bytesAfter: 0 } : null));
  const name = (id: string) => {
    const a = accounts.find((x) => x.id === id);
    return a ? accountName(a, accounts) : id;
  };
  const target = change.kind === "window" ? change.months : null;

  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="st-confirm sw-confirm" role="alertdialog" aria-modal="true" aria-label={plan?.title ?? "Confirm sync change"}>
        <h3>{plan?.title ?? "Sync change"}</h3>
        {plan ? (
          <p className="st-muted sw-summary">{plan.summary}</p>
        ) : (
          <p className="st-muted sw-summary" role="status">
            <span className="spinner" aria-hidden="true" /> Calculating…
          </p>
        )}
        {shrink && target != null && preview && preview.messages > 0 ? (
          <label className="sw-check">
            <input type="checkbox" checked={freeUp} onChange={(e) => setFreeUp(e.target.checked)} />
            <span>
              Also free up space: remove bodies older than {windowLabel(target).toLowerCase()} (≈{bytes(preview.bodyBytes)}; headers stay searchable)
            </span>
          </label>
        ) : null}
        {previewError ? <p className="st-muted is-warn">Couldn't size the older mail: {previewError}</p> : null}
        {plan && plan.lines.some((l) => l.messages > 0) ? (
          <details className="sw-details">
            <summary>Per account</summary>
            <table className="sw-table tnum">
              <tbody>
                {plan.lines.map((l) => (
                  <tr key={l.accountId}>
                    <td className="truncate">{name(l.accountId)}</td>
                    <td>{num(l.messages)} messages</td>
                    <td className="st-muted">{l.messages > 0 ? eta(l.secs) : "done"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        ) : null}
        <div className="st-confirm-actions">
          <button ref={cancelRef} className="btn btn-ghost" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!plan} onClick={() => onConfirm(shrink && freeUp)}>
            {plan?.confirm ?? "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
