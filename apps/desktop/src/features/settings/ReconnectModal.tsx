// Reconnect an account that needs signing in again. Google refuses OAuth in
// embedded webviews, so the sign-in itself happens in the browser; this modal
// keeps the app side of it in view: waiting → reconnected (auto-closes) or an
// inline error with Retry. Mounted once by App; anyone opens it with
// openReconnect(accountId). OWNER: settings agent.
import { useEffect, useRef, useSyncExternalStore } from "react";
import { api, asCommandError } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { AccountDot } from "../../components/Identity";
import { SignInLinkHelp } from "../../components/SignInLinkHelp";
import { accountById, setAccounts } from "../../app/store";
import "./settings.css";

type Phase = { kind: "waiting" } | { kind: "done" } | { kind: "error"; message: string };

interface State {
  accountId: string | null;
  phase: Phase;
  /** Bumped per attempt so a stale result can't land on a newer one. */
  attempt: number;
}

let state: State = { accountId: null, phase: { kind: "waiting" }, attempt: 0 };
const subs = new Set<() => void>();
const set = (p: Partial<State>) => {
  state = { ...state, ...p };
  subs.forEach((f) => f());
};

const DONE_CLOSE_MS = 1000;

async function run(accountId: string) {
  const attempt = state.attempt + 1;
  set({ accountId, phase: { kind: "waiting" }, attempt });
  try {
    await api.reconnectAccount(accountId);
    if (state.attempt !== attempt) return;
    set({ phase: { kind: "done" } });
    // Refresh names/colors (reconnect may update the display name).
    api.listAccounts().then(setAccounts, () => {});
    setTimeout(() => {
      if (state.attempt === attempt && state.phase.kind === "done") set({ accountId: null });
    }, DONE_CLOSE_MS);
  } catch (e) {
    if (state.attempt !== attempt) return;
    const err = asCommandError(e);
    if (err.code === "cancelled") return; // closed by Cancel; nothing to show
    set({ phase: { kind: "error", message: err.message } });
  }
}

/** Start the browser sign-in for `accountId` and show its progress. */
export function openReconnect(accountId: string): void {
  void run(accountId);
}

function cancel() {
  const waiting = state.phase.kind === "waiting";
  set({ accountId: null, attempt: state.attempt + 1 });
  if (waiting) void api.cancelSignIn().catch(() => {});
}

export function ReconnectModal() {
  const s = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => state,
  );
  const primaryRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!s.accountId) return;
    primaryRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      // Own the keyboard while open: Esc cancels, nothing reaches mail shortcuts.
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
      if (e.key !== "Tab" && e.key !== "Enter" && e.key !== " ") e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [s.accountId, s.phase.kind]);

  if (!s.accountId) return null;
  const account = accountById(s.accountId);
  const email = account?.email ?? s.accountId;
  const accountId = s.accountId;

  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && cancel()}>
      <div className="st-confirm st-reconnect" role="dialog" aria-modal="true" aria-label={`Reconnect ${email}`}>
        <div className="st-reconnect-account">
          <AccountDot color={account?.color} />
          <span className="truncate">{email}</span>
        </div>
        {s.phase.kind === "waiting" ? (
          <>
            <h3 className="row-flex">
              <span className="st-spinner" aria-hidden="true" />
              Finish signing in in your browser…
            </h3>
            <SignInLinkHelp />
            <div className="st-confirm-actions">
              <button ref={primaryRef} className="btn btn-ghost" onClick={cancel}>
                Cancel
              </button>
            </div>
          </>
        ) : s.phase.kind === "done" ? (
          <h3 className="row-flex st-reconnect-done" role="status">
            <Icon name="check" size="sm" />
            Reconnected
          </h3>
        ) : (
          <>
            <h3>Couldn't reconnect</h3>
            <p className="st-error" role="alert">
              {s.phase.message}
            </p>
            <div className="st-confirm-actions">
              <button className="btn btn-ghost" onClick={cancel}>
                Close
              </button>
              <button ref={primaryRef} className="btn btn-primary" onClick={() => void run(accountId)}>
                Retry
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
