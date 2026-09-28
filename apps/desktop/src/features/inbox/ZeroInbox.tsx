// Inbox zero: a penguin on an ice floe (design/05-command.html). With the
// Split Inbox, one split can be clear while others aren't: then it says so
// and offers the next split with mail (Tab). The celebration (the penguin
// landing, a few flakes, how much you cleared today) can be turned off in
// Settings → Inbox; then it's a plain empty state.
import { useMemo } from "react";
import { useUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { useSetting } from "../../lib/settings";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { canUndo, undo } from "../../app/actions";
import { currentSplit, meta } from "../../app/store";
import { nextWithMail, splitTabs } from "../../app/splits";
import { setSplit } from "../split/state";
import { clearedToday } from "../zero/zero";
import "../zero/zero.css";

export function ZeroInbox() {
  useUi((s) => `${s.split}|${s.view.kind}`);
  const splitsOn = useSetting("inboxTabs");
  const splits = useSetting("inboxSplits");
  const celebrate = useSetting("zeroCelebration");
  const counts = meta.use((m) => m.splitCounts);
  const split = splitsOn ? currentSplit() : null;
  const tabs = splitTabs(splits);
  const here = split === null ? null : tabs.find((t) => t.id === split) ?? null;
  const next = split === null ? null : nextWithMail(tabs, counts, split);
  const cleared = useMemo(() => clearedToday(), []);
  // The whole inbox is empty unless another split still holds mail.
  const allClear = next === null;

  if (!celebrate) {
    return (
      <div className="empty">
        <div className="empty-title">{here && !allClear ? `Nothing in ${here.name}` : "No mail in your inbox"}</div>
        {next && (
          <button className="btn btn-secondary empty-action" onClick={() => setSplit(next.id)}>
            {next.name}
            <Kbd>⇥</Kbd>
          </button>
        )}
      </div>
    );
  }

  return (
    <div className={"zero" + (allClear ? " is-celebrating" : "")}>
      <div className="zero-band dotgrid" />
      {allClear && <Flakes />}
      <div className="zero-inner">
        <div className="zero-art">
          <div className="zero-bloom" />
          <svg viewBox="14 24 92 92" width="120" height="120" aria-hidden="true">
            <path d="M14 94c10-4 28-6 46-6s36 2 46 6l-8 9c-12 3-26 4-38 4s-26-1-38-4Z" fill="var(--floe)" stroke="var(--border-default)" />
            <path d="M26 112h18M56 114h26M90 111h10" stroke="var(--border-default)" strokeWidth="2" strokeLinecap="round" />
            <ellipse cx="47" cy="70" rx="5" ry="13" transform="rotate(18 47 70)" fill="var(--peng)" />
            <ellipse cx="73" cy="70" rx="5" ry="13" transform="rotate(-18 73 70)" fill="var(--peng)" />
            <path d="M60 32c-12 0-19 10-19 26 0 18 7 32 19 32s19-14 19-32c0-16-7-26-19-26Z" fill="var(--peng)" stroke="var(--peng-edge)" />
            <path d="M60 46c-4-4-12-3-12 5 0 5 1 10 2 15 2 10 5 19 10 19s8-9 10-19c1-5 2-10 2-15 0-8-8-9-12-5Z" fill="#f5f5f4" />
            <circle cx="55" cy="51" r="1.8" fill="#0b0c0e" />
            <circle cx="65" cy="51" r="1.8" fill="#0b0c0e" />
            <path d="M57 55h6l-3 4Z" fill="#ff9d4d" />
            <ellipse cx="54" cy="90" rx="5" ry="2.2" fill="#ff9d4d" />
            <ellipse cx="66" cy="90" rx="5" ry="2.2" fill="#ff9d4d" />
          </svg>
        </div>
        <h2 className="zero-title">{allClear ? "Inbox zero. Nicely done." : `${here?.name ?? "This split"} is clear.`}</h2>
        <p className="zero-copy">
          {allClear
            ? "Everything here is handled. New mail shows up the moment it syncs."
            : `${next!.name} still has ${num(counts?.[next!.id]?.total ?? 0)} to go.`}
        </p>
        {allClear && cleared > 0 && (
          <p className="zero-stat">
            You cleared <strong>{num(cleared)}</strong> {cleared === 1 ? "conversation" : "conversations"} today.
          </p>
        )}
        <div className="zero-cta">
          {next && (
            <button className="btn btn-secondary zero-next" onClick={() => setSplit(next.id)}>
              Go to {next.name}
              <Kbd>⇥</Kbd>
            </button>
          )}
          {canUndo() && (
            <button className="btn btn-ghost" onClick={undo}>
              <Icon name="undo" size="sm" />
              Undo last done
              <Kbd>Z</Kbd>
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** A dozen flakes drifting down once (CSS only; none with reduced motion). */
function Flakes() {
  const flakes = useMemo(
    () =>
      Array.from({ length: 14 }, (_, i) => ({
        left: `${(i * 37 + 11) % 100}%`,
        delay: `${((i * 53) % 900) / 1000}s`,
        dx: `${((i * 29) % 40) - 20}px`,
        size: 4 + ((i * 7) % 4),
      })),
    [],
  );
  return (
    <div className="zero-flakes" aria-hidden="true">
      {flakes.map((f, i) => (
        <span
          key={i}
          className="zero-flake"
          style={{ left: f.left, animationDelay: f.delay, width: f.size, height: f.size, ["--dx" as string]: f.dx }}
        />
      ))}
    </div>
  );
}
