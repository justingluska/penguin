// Settings → Accounts → Pair iPhone (docs/PAIRING.md): a QR code the iPhone
// app scans; when a phone connects, it is named with a six-digit code to
// compare and nothing is sent until Send. Progress comes from
// penguin://pair-iphone; closing the dialog cancels the pairing (the Mac
// stops listening). Tokens never reach this side, only addresses.
// OWNER: settings agent.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, asCommandError, onPairIphone } from "../../lib/api";
import type { PairAccount, PairStart } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { AccountDot } from "../../components/Identity";
import { formatLeft, nextPhase, pairingLive, plural, sendLabel, statusLabel, type PairPhase as Phase } from "./pairModel";
import "./pair-iphone.css";

/** The row in Settings → Accounts. */
export function PairIphoneRow() {
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return (
    <div className="setting-row st-pair-row" data-setting="pair-iphone">
      <div className="min0 grow">
        <span className="setting-label">Pair iPhone</span>
        <p className="st-muted">Move these accounts to Penguin for iPhone: the phone scans a code, nothing goes through a server.</p>
      </div>
      <button className="btn btn-sm" onClick={() => setOpen(true)}>
        <Icon name="phone" size="xs" />
        Show code
      </button>
      {open ? <PairIphoneDialog onClose={close} /> : null}
    </div>
  );
}

export function PairIphoneDialog({ onClose }: { onClose: () => void }) {
  const [phase, setPhase] = useState<Phase>({ kind: "starting" });
  const [attempt, setAttempt] = useState(0);
  const session = useRef<string | null>(null);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

  // Start (and on Try again, start over); the backend cancels any older one.
  useEffect(() => {
    let live = true;
    setPhase({ kind: "starting" });
    api.pairIphoneStart().then(
      (start) => {
        if (!live) {
          void api.pairIphoneCancel(start.sessionId).catch(() => {});
          return;
        }
        session.current = start.sessionId;
        setPhase({ kind: "showing", start });
      },
      (e) => live && setPhase({ kind: "error", message: asCommandError(e).message }),
    );
    return () => {
      live = false;
    };
  }, [attempt]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let live = true;
    void onPairIphone((e) => {
      if (e.sessionId !== session.current) return;
      setPhase((p) => nextPhase(p, e));
    }).then((u) => (live ? (unlisten = u) : u()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  // Closing stops the listener unless the pairing already ended.
  useEffect(
    () => () => {
      const id = session.current;
      if (id && pairingLive(phaseRef.current)) void api.pairIphoneCancel(id).catch(() => {});
    },
    [],
  );

  const close = onClose;
  const primary = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    primary.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [phase.kind, onClose]);

  const answer = (send: boolean) => {
    if (phase.kind !== "connected" || !session.current) return;
    setPhase({ ...phase, answered: true });
    api.pairIphoneConfirm(session.current, send).catch((e) => setPhase({ kind: "error", message: asCommandError(e).message }));
  };

  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="st-confirm st-pair" role="dialog" aria-modal="true" aria-label="Pair iPhone">
        {phase.kind === "starting" ? (
          <h3 className="row-flex">
            <span className="st-spinner" aria-hidden="true" />
            Preparing a pairing code…
          </h3>
        ) : phase.kind === "showing" ? (
          <Showing start={phase.start} onCancel={close} primary={primary} />
        ) : phase.kind === "connected" || phase.kind === "sending" ? (
          <Connected phase={phase} onAnswer={answer} primary={primary} />
        ) : phase.kind === "done" ? (
          <Done phase={phase} onClose={close} primary={primary} />
        ) : phase.kind === "declined" ? (
          <>
            <h3>Nothing was sent</h3>
            <p className="st-muted">The iPhone was told no. Pair again whenever you like.</p>
            <div className="st-confirm-actions">
              <button ref={primary} className="btn btn-primary" onClick={close}>
                Close
              </button>
            </div>
          </>
        ) : (
          <>
            <h3>Pairing stopped</h3>
            <p className="st-error" role="alert">
              {phase.message}
            </p>
            <div className="st-confirm-actions">
              <button className="btn btn-ghost" onClick={close}>
                Close
              </button>
              <button ref={primary} className="btn btn-primary" onClick={() => setAttempt((n) => n + 1)}>
                Try again
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function Showing({ start, onCancel, primary }: { start: PairStart; onCancel: () => void; primary: React.RefObject<HTMLButtonElement | null> }) {
  const left = useCountdown(start.expiresAt);
  const moving = start.accounts.filter((a) => a.status === "token");
  const signIn = start.accounts.filter((a) => a.status === "signIn");
  const pad = 4;
  const box = start.qr.size + pad * 2;
  return (
    <>
      <h3>Pair iPhone</h3>
      <div className="st-pair-body">
        <svg
          className="st-pair-qr"
          viewBox={`${-pad} ${-pad} ${box} ${box}`}
          shapeRendering="crispEdges"
          role="img"
          aria-label="Pairing code for Penguin on iPhone"
        >
          {/* Black on white in both themes: what cameras read best. */}
          <rect x={-pad} y={-pad} width={box} height={box} fill="#fff" />
          <path d={start.qr.path} fill="#000" />
        </svg>
        <ol className="st-pair-steps">
          <li>Open Penguin on your iPhone.</li>
          <li>
            Tap <b>Pair with Mac</b> (on the welcome screen, or in Mailboxes).
          </li>
          <li>Point the camera at this code.</li>
        </ol>
      </div>
      <p className="st-muted">Both need to be on the same Wi-Fi network. The code works once, for {formatLeft(left)}.</p>
      <AccountSummary moving={moving} signIn={signIn} />
      <p className="st-muted st-pair-where">
        This Mac: {start.hosts[0]}:{start.port}
      </p>
      <div className="st-confirm-actions">
        <button ref={primary} className="btn btn-ghost" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </>
  );
}

function AccountSummary({ moving, signIn }: { moving: PairAccount[]; signIn: PairAccount[] }) {
  if (moving.length + signIn.length === 0) {
    return <p className="st-muted">There are no accounts on this Mac to move.</p>;
  }
  return (
    <details className="st-pair-summary">
      <summary>
        {moving.length > 0 ? `${plural(moving.length, "account")} can move` : "No sign-ins can move"}
        {signIn.length > 0 ? ` · ${signIn.length} to sign in on the iPhone` : ""}
      </summary>
      <AccountList accounts={[...moving, ...signIn]} />
    </details>
  );
}

function AccountList({ accounts }: { accounts: PairAccount[] }) {
  return (
    <ul className="st-pair-accounts">
      {accounts.map((a) => (
        <li key={a.email}>
          <AccountDot color={a.color} />
          <span className="grow min0">
            <span className="truncate st-pair-email">{a.email}</span>
            {a.reason ? <span className="st-muted st-pair-reason">{a.reason}</span> : null}
          </span>
          <span className={"st-pair-status is-" + a.status}>{statusLabel(a.status)}</span>
        </li>
      ))}
    </ul>
  );
}

function Connected({
  phase,
  onAnswer,
  primary,
}: {
  phase: Extract<Phase, { kind: "connected" | "sending" }>;
  onAnswer: (send: boolean) => void;
  primary: React.RefObject<HTMLButtonElement | null>;
}) {
  const phone = phase.phone;
  const detail = [phone.model, phone.system].filter(Boolean).join(" · ");
  if (phase.kind === "sending") {
    return (
      <h3 className="row-flex" role="status">
        <span className="st-spinner" aria-hidden="true" />
        Sending to {phone.name}…
      </h3>
    );
  }
  return (
    <>
      <h3>{phone.name} wants your accounts</h3>
      {detail ? <p className="st-muted">{detail}</p> : null}
      <p className="st-pair-code">
        Check that the iPhone shows <b className="tnum">{phase.code}</b>
      </p>
      {phase.clientNote ? <p className="st-pair-note">{phase.clientNote}</p> : null}
      <AccountList accounts={phase.accounts} />
      <p className="st-muted">If this isn't your iPhone, or the numbers differ, don't send.</p>
      <div className="st-confirm-actions">
        <button className="btn btn-ghost" disabled={phase.answered} onClick={() => onAnswer(false)}>
          Don't send
        </button>
        <button ref={primary} className="btn btn-primary" disabled={phase.answered} onClick={() => onAnswer(true)}>
          {sendLabel(phase.accounts)}
        </button>
      </div>
    </>
  );
}

function Done({ phase, onClose, primary }: { phase: Extract<Phase, { kind: "done" }>; onClose: () => void; primary: React.RefObject<HTMLButtonElement | null> }) {
  return (
    <>
      <h3 className="row-flex st-pair-done" role="status">
        <Icon name="check" size="sm" />
        {phase.imported.length > 0 ? `Added ${plural(phase.imported.length, "account")} to ${phase.phoneName}` : `Sent to ${phase.phoneName}`}
      </h3>
      {phase.imported.length > 0 ? <p className="st-muted">{phase.imported.join(", ")}</p> : null}
      {phase.signIn.length > 0 ? (
        <p className="st-muted">
          The iPhone lists {phase.signIn.length === 1 ? "one more account" : `${phase.signIn.length} more accounts`} with a Sign in button: {phase.signIn.join(", ")}.
        </p>
      ) : null}
      {phase.failed.length > 0 ? (
        <ul className="st-pair-failed">
          {phase.failed.map((f) => (
            <li key={f.email} className="st-error">
              {f.email}: {f.reason}
            </li>
          ))}
        </ul>
      ) : null}
      <div className="st-confirm-actions">
        <button ref={primary} className="btn btn-primary" onClick={onClose}>
          Done
        </button>
      </div>
    </>
  );
}

function useCountdown(until: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return Math.max(0, until - now);
}
