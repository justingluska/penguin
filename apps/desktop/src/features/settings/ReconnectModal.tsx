// Reconnect an account that needs signing in again. Google and Microsoft
// refuse OAuth in embedded webviews, so their sign-in happens in the browser;
// this modal keeps the app side of it in view: waiting → reconnected
// (auto-closes) or an inline error with Retry. An IMAP account (Yahoo, AOL,
// iCloud, …) signs in with a password, which the provider revoked or
// refused, so the modal asks for a new one with the provider's app-password
// guide, and keeps the field open with the server's answer until one works.
// Mounted once by App; anyone opens it with openReconnect(accountId).
// OWNER: settings agent.
import { useEffect, useRef, useState, useSyncExternalStore, type FormEvent } from "react";
import { api, asCommandError } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { AccountDot } from "../../components/Identity";
import { SignInLinkHelp } from "../../components/SignInLinkHelp";
import { accountById, setAccounts } from "../../app/store";
import { cleanAppPassword, reconnectGuide } from "../onboarding/providers";
import type { Account } from "../../lib/types";
import { busSend, isMainWindow } from "../../lib/windowBus";
import { MAIN_LABEL } from "../../lib/windowRoute";
import "./settings.css";

type Phase =
  | { kind: "waiting" }
  | { kind: "done" }
  | { kind: "error"; message: string }
  /** IMAP: asking for a new password; `error` = why the last one failed. */
  | { kind: "password"; error: string | null };

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

/** Signs in with a password rather than through the browser. */
function usesPassword(account: Account | undefined): boolean {
  return account?.provider === "imap";
}

async function run(accountId: string, password?: string) {
  const attempt = state.attempt + 1;
  set({ accountId, phase: { kind: "waiting" }, attempt });
  try {
    await api.reconnectAccount(accountId, password);
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
    if (password !== undefined) set({ phase: { kind: "password", error: err.message } });
    else set({ phase: { kind: "error", message: err.message } });
  }
}

/** Start signing `accountId` in again (browser, or a new password for IMAP) and show its progress. */
export function openReconnect(accountId: string): void {
  // Signing in again happens in the main window (app/handoffs.ts).
  if (!isMainWindow) {
    void busSend(MAIN_LABEL, "reconnect", { accountId });
    return;
  }
  if (usesPassword(accountById(accountId))) {
    set({ accountId, phase: { kind: "password", error: null }, attempt: state.attempt + 1 });
    return;
  }
  void run(accountId);
}

function cancel() {
  // Only a browser sign-in can be abandoned; a password check just finishes unseen.
  const browser = state.phase.kind === "waiting" && !usesPassword(accountById(state.accountId ?? ""));
  set({ accountId: null, attempt: state.attempt + 1 });
  if (browser) void api.cancelSignIn().catch(() => {});
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
  const passwordRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!s.accountId) return;
    (passwordRef.current ?? primaryRef.current)?.focus();
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
  const password = usesPassword(account);
  const guide = password ? reconnectGuide(account?.providerConfig) : null;
  const service = guide?.name ?? "The mail server";

  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && cancel()}>
      <div className="st-confirm st-reconnect" role="dialog" aria-modal="true" aria-label={`Reconnect ${email}`}>
        <div className="st-reconnect-account">
          <AccountDot color={account?.color} />
          <span className="truncate">{email}</span>
        </div>
        {s.phase.kind === "password" ? (
          <PasswordStep
            key={s.attempt}
            email={email}
            service={service}
            guide={guide}
            error={s.phase.error}
            inputRef={passwordRef}
            onCancel={cancel}
            onSubmit={(pw) => void run(accountId, pw)}
          />
        ) : s.phase.kind === "waiting" && password ? (
          <>
            <h3 className="row-flex">
              <span className="st-spinner" aria-hidden="true" />
              Checking with {guide?.name ?? "the server"}…
            </h3>
            <div className="st-confirm-actions">
              <button ref={primaryRef} className="btn btn-ghost" onClick={cancel}>
                Cancel
              </button>
            </div>
          </>
        ) : s.phase.kind === "waiting" ? (
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

function PasswordStep({
  email,
  service,
  guide,
  error,
  inputRef,
  onCancel,
  onSubmit,
}: {
  email: string;
  service: string;
  guide: ReturnType<typeof reconnectGuide>;
  error: string | null;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onCancel: () => void;
  onSubmit: (password: string) => void;
}) {
  const [value, setValue] = useState("");
  // App passwords are shown in groups ("abcd efgh …"); the server wants them joined.
  const typed = guide ? cleanAppPassword(value) : value;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (typed) onSubmit(typed);
  };
  return (
    <form onSubmit={submit}>
      <h3>{guide ? `Enter a new ${guide.field.toLowerCase()}` : "Enter your password again"}</h3>
      <p className="st-reconnect-lead">
        {service} refused the password Penguin saved for {email}.
        {guide && guide.name !== "Proton Mail"
          ? ` ${guide.name} revokes app passwords when you change your password or remove them, so make a new one for Penguin.`
          : ""}
      </p>
      {guide && (
        <button type="button" className="btn btn-secondary btn-sm st-reconnect-guide" onClick={() => void api.openExternal(guide.page.url).catch(() => {})}>
          {guide.page.label} <Icon name="external" size="xs" />
        </button>
      )}
      <label className="st-reconnect-field">
        <span>{guide?.field ?? "Password"}</span>
        <input
          ref={inputRef}
          className="input"
          type="password"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={guide?.placeholder}
          autoComplete="off"
          spellCheck={false}
          aria-invalid={!!error}
        />
      </label>
      {error && (
        <p className="st-error" role="alert">
          {error}
        </p>
      )}
      <div className="st-confirm-actions">
        <button type="button" className="btn btn-ghost" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={!typed}>
          Reconnect
        </button>
      </div>
    </form>
  );
}
