// "Sign in with Google": the button, the calendar box, and behind "Having
// trouble?" a drawn walkthrough of the browser screens (unverified-app
// warning included) and troubleshooting. Shared by the setup guide and the
// Add account modal (AddAccountModal.tsx). SyncRow: one account's live sync.
import { useEffect, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { Account, SyncStatus } from "../../lib/types";
import { ago, num } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { DEFAULT_SETTINGS, updateSettings, useSetting } from "../../lib/settings";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { serviceName } from "../../lib/capabilities";
import { openReconnect } from "../settings/ReconnectModal";
import { SyncFixButton, syncFixFor } from "../settings/syncFix";
import { Troubleshooting, type Problem } from "./parts";
import { links } from "./links";
import { SignInLinkHelp } from "../../components/SignInLinkHelp";
import { notePendingError } from "./pending";

/** A sign-in error in plain words; null when the user cancelled. */
export function signInErrorText(e: unknown): string | null {
  const err = asCommandError(e);
  if (err.code === "cancelled") return null;
  if (err.code === "notConfigured") return "Add your Google client JSON first (the “Add the client” step).";
  if (err.code === "network") return `Couldn't reach Google. Check your connection and try again. (${err.message})`;
  const m = err.message;
  if (/redirect_uri_mismatch/i.test(m)) return "Google rejected the sign-in address. The client JSON is probably a Web application client; create a Desktop app client (the “Create the client” step).";
  if (/admin_policy_enforced/i.test(m))
    return "This account's Google Workspace admin blocks the app, or blocks Google Calendar for it. Untick \"Also connect Google Calendar\" and try again, or have the admin trust the app (see Troubleshooting).";
  if (/org_internal/i.test(m)) return "The app's audience is Internal, so accounts outside its organization can't sign in. Switch the audience to External (the “Branding” step).";
  if (/access_denied|cancelled in the browser/i.test(m)) return "Sign-in was cancelled in the browser. Try again when you're ready.";
  return m.charAt(0).toUpperCase() + m.slice(1);
}

export function accountProblems(projectId: string): Problem[] {
  return [
    {
      symptom: (
        <>
          Browser shows <code>Access blocked</code> or <code>org_internal</code>
        </>
      ),
      fix: (
        <>
          The app's audience is <b>Internal</b>, which only allows accounts in the project's own Workspace organization. In Google Auth Platform →{" "}
          <a href={links.audience(projectId)} onClick={openLink}>Audience</a>, choose <b>Make external</b>, then publish it. If it says the app is in
          Testing, publish it (the “Audience &amp; publish” step) or add yourself as a test user.
        </>
      ),
    },
    {
      symptom: (
        <>
          <code>admin_policy_enforced</code> or "Your administrator has blocked this app"
        </>
      ),
      fix: (
        <>
          The account's Google Workspace admin must allow Penguin. In the{" "}
          <a href={links.adminAppAccess()} onClick={openLink}>Admin console</a>: Security → Access and data control → API controls → Manage app
          access → Configure new app, paste the client ID (Google Auth Platform → Clients), and set it to <b>Trusted</b>. Then sign in again. If
          the admin allows Gmail but not Calendar, untick <b>Also connect Google Calendar</b> above and sign in with mail only.
        </>
      ),
    },
    {
      symptom: 'No "Advanced" link on the "Google hasn\'t verified this app" screen',
      fix: "The account has Advanced Protection or an admin policy that blocks unverified apps. A Workspace admin can trust the client ID (see above); a personal account with Advanced Protection can't use a private app.",
    },
    {
      symptom: (
        <>
          <code>redirect_uri_mismatch</code> or "can't connect to 127.0.0.1"
        </>
      ),
      fix: "redirect_uri_mismatch means the JSON is a Web application client. Create a Desktop app client (the “Create the client” step) and add its JSON instead. A 127.0.0.1 error means Penguin stopped listening: keep Penguin open and start the sign-in again.",
    },
    {
      symptom: '"Gmail access wasn\'t granted"',
      fix: 'Google lets you untick permissions. Sign in again and make sure "Read, compose, and send emails from your Gmail account" is ticked.',
    },
    {
      symptom: "Signed in, but the calendar isn't connected",
      fix: "The calendar box was unticked on Google's screen, which is fine: mail works without it. Connect it any time in Settings → Calendar.",
    },
    {
      symptom: (
        <>
          Signed out after about 7 days (<code>invalid_grant</code>)
        </>
      ),
      fix: (
        <>
          The app is still in Testing. Open <a href={links.audience(projectId)} onClick={openLink}>Audience</a>, click <b>Publish app</b>, then use
          Reconnect on each account.
        </>
      ),
    },
  ];
}

function openLink(e: React.MouseEvent<HTMLAnchorElement>) {
  e.preventDefault();
  void api.openExternal(e.currentTarget.href);
}

/**
 * Google's step: one Sign in button and the calendar box. The browser
 * walkthrough and troubleshooting wait behind "Having trouble?". Success is
 * shown by the flow's Done step (onAdded), not here.
 */
export function AccountsPanel({
  allAccounts,
  onAdded,
  projectId,
  autoFocus,
  sheet,
  loginHint,
}: {
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  projectId: string;
  autoFocus?: boolean;
  /** An iOS client is set, so sign-in uses the macOS sheet, not a browser tab. */
  sheet?: boolean;
  /** The address typed in Add account: Google preselects it until it's added. */
  loginHint?: string;
}) {
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const attempt = useRef(0);
  const attemptPending = useRef(false);
  const signInRef = useRef<HTMLButtonElement>(null);
  const calendarOn = useSetting("calendar")?.connectOnSignIn ?? true;

  useEffect(() => {
    if (autoFocus) signInRef.current?.focus();
  }, [autoFocus]);

  // Leaving mid sign-in abandons it, so its loopback listener closes.
  useEffect(
    () => () => {
      if (attemptPending.current) void api.cancelSignIn().catch(() => {});
    },
    [],
  );

  async function signIn() {
    const id = ++attempt.current;
    setError(null);
    setWaiting(true);
    attemptPending.current = true;
    try {
      const acc = await api.addAccount(hint);
      if (id !== attempt.current) return;
      onAdded(acc);
    } catch (e) {
      if (id !== attempt.current) return;
      const text = signInErrorText(e);
      setError(text);
      if (text && hint) notePendingError(hint, text);
    } finally {
      if (id === attempt.current) {
        setWaiting(false);
        attemptPending.current = false;
      }
    }
  }

  function cancel() {
    attempt.current++;
    attemptPending.current = false;
    setWaiting(false);
    void api.cancelSignIn().catch(() => {});
  }

  const hint = loginHint && !allAccounts.some((a) => a.email.toLowerCase() === loginHint.toLowerCase()) ? loginHint : null;

  return (
    <div className="onb-accounts">
      <div className="onb-signin">
        {waiting ? (
          <>
            <span className="onb-wait" role="status">
              <span className="onb-spin" aria-hidden="true" />
              {sheet ? "Finish signing in in the sign-in sheet…" : "Finish signing in in your browser…"}
            </span>
            <span className="grow" />
            <button className="btn btn-secondary btn-sm" onClick={cancel}>
              Cancel
            </button>
          </>
        ) : (
          <button ref={signInRef} className="btn btn-primary onb-google-btn" onClick={() => void signIn()}>
            <GoogleG />
            Sign in with Google
          </button>
        )}
      </div>
      {waiting && <SignInLinkHelp />}
      <CalendarOption disabled={waiting} />
      {error && (
        <div className="onb-error" role="alert">
          <Icon name="info" size="xs" />
          <span className="grow">Sign-in didn't finish. {error}</span>
        </div>
      )}
      {helpOpen ? (
        <>
          <BrowserWalkthrough sheet={!!sheet} calendar={calendarOn} />
          <Troubleshooting problems={accountProblems(projectId)} open />
        </>
      ) : (
        <button type="button" className="onb-textbtn onb-help-link" onClick={() => setHelpOpen(true)}>
          Having trouble?
        </button>
      )}
    </div>
  );
}

/**
 * "Also connect Google Calendar": the `calendar.connectOnSignIn` setting
 * (also in Settings → Calendar). On, add_account asks for read-only calendar
 * in the same Google consent and starts its sync.
 */
function CalendarOption({ disabled }: { disabled: boolean }) {
  const prefs = useSetting("calendar") ?? DEFAULT_SETTINGS.calendar;
  function set(connectOnSignIn: boolean) {
    updateSettings({ calendar: { ...prefs, connectOnSignIn } }).catch((e) =>
      toast({ tone: "error", message: asCommandError(e).message }),
    );
  }
  return (
    <label className="onb-cal-check">
      <input type="checkbox" checked={prefs.connectOnSignIn} disabled={disabled} onChange={(e) => set(e.target.checked)} />
      <span>Also connect Calendar (read-only)</span>
    </label>
  );
}

// ---------------------------------------------------------------------------
// What happens in the browser, drawn (not real Google screenshots).
// ---------------------------------------------------------------------------

function BrowserWalkthrough({ sheet, calendar }: { sheet: boolean; calendar: boolean }) {
  return (
    <details className="onb-walk" open>
      <summary>
        <Icon name="right" size="xs" className="onb-trouble-chev" />
        {sheet ? "What you'll see in the sign-in sheet" : "What you'll see in the browser"}
      </summary>
      <ol className="onb-walk-grid">
        <li>
          <Mini url="accounts.google.com">
            <div className="mini-title">Choose an account</div>
            <div className="mini-acct is-pick">
              <i className="mini-avatar" />
              <span className="mini-lines">
                <b />
                <span />
              </span>
            </div>
            <div className="mini-acct">
              <i className="mini-avatar" />
              <span className="mini-lines">
                <b />
                <span />
              </span>
            </div>
          </Mini>
          <p>
            <b>Pick the account</b> to add. Signing in to it is Google's normal flow.
          </p>
        </li>
        <li>
          <Mini url="accounts.google.com">
            <div className="mini-warn">!</div>
            <div className="mini-title">Google hasn't verified this app</div>
            <div className="mini-bar" />
            <div className="mini-bar short" />
            <div className="mini-link is-hot">Advanced</div>
            <div className="mini-link is-hot">Go to Penguin (unsafe)</div>
          </Mini>
          <p>
            Expected: it's <b>your</b> private app. Click <b>Advanced</b>, then <b>Go to Penguin (unsafe)</b>.
          </p>
        </li>
        <li>
          <Mini url="accounts.google.com">
            <div className="mini-title">Penguin wants access to your Google Account</div>
            <div className="mini-check is-hot">
              <i className="mini-box">
                <Icon name="check" size="2xs" />
              </i>
              Read, compose, and send emails from your Gmail account
            </div>
            {calendar && (
              <div className="mini-check">
                <i className="mini-box">
                  <Icon name="check" size="2xs" />
                </i>
                See and download any calendar you can access
              </div>
            )}
            <div className="mini-foot">
              <span className="mini-btn">Continue</span>
            </div>
          </Mini>
          <p>
            <b>Tick the Gmail permission</b> (Google may leave it unticked){calendar ? <>, and the calendar one if you want it</> : null}, then{" "}
            <b>Continue</b>.
          </p>
        </li>
        <li>
          <Mini url={sheet ? "Penguin" : "127.0.0.1"}>
            <div className="mini-ok">
              <Icon name="check" size="xs" />
            </div>
            <div className="mini-title center">You're signed in</div>
            <div className="mini-bar center" />
          </Mini>
          <p>
            {sheet ? (
              <>
                <b>The sheet closes itself.</b> The account appears here and starts syncing.
              </>
            ) : (
              <>
                <b>Close the tab.</b> The account appears here and starts syncing.
              </>
            )}
          </p>
        </li>
      </ol>
    </details>
  );
}

function Mini({ url, children }: { url: string; children: React.ReactNode }) {
  return (
    <div className="mini" aria-hidden="true">
      <div className="mini-chrome">
        <i />
        <i />
        <i />
        <span className="mini-url">
          <Icon name="lock" size="2xs" />
          {url}
        </span>
      </div>
      <div className="mini-page">{children}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// One account's sync progress
// ---------------------------------------------------------------------------

/** `provider`: who the account syncs with, for the status text (default Gmail). */
export function SyncRow({ account, name, status, provider }: { account: Account; name: string; status?: SyncStatus; provider?: string }) {
  const tone = accountTone(account.color);
  const phase = status?.phase;
  let badge = <span className="badge t-gray">Starting</span>;
  const service = provider ?? serviceName(account);
  let body = <div className="sync-phase">Connecting to {service}…</div>;

  if (status && phase === "backfilling") {
    const total = status.totalEstimate;
    const pct = total ? Math.min(100, Math.round((status.indexed / total) * 100)) : null;
    badge = <span className="badge t-gray">Syncing</span>;
    body = (
      <>
        <div className="sync-phase">
          <span className="tnum emph">
            {num(status.indexed)}
            {total ? ` / ${num(total)}` : ""}
          </span>
          <span className="muted">messages</span>
          <span className="grow" />
          {status.ratePerMin ? <span className="tnum muted">{num(status.ratePerMin)}/min</span> : null}
          {pct !== null && <span className="tnum muted">{pct}%</span>}
        </div>
        <div className="sync-progress" role="progressbar" aria-label={`${name} sync`} aria-valuemin={0} aria-valuemax={total ?? undefined} aria-valuenow={status.indexed}>
          <span style={{ width: `${pct ?? 8}%` }} />
        </div>
      </>
    );
  } else if (status && (phase === "incremental" || phase === "idle")) {
    badge = (
      <span className="badge t-green">
        <Icon name="check" size="xs" />
        Up to date
      </span>
    );
    body = (
      <div className="sync-phase">
        <span className="tnum">{num(status.indexed)} messages</span>
        <span className="grow" />
        {status.lastSyncedAt && <span className="muted">{ago(status.lastSyncedAt)}</span>}
      </div>
    );
  } else if (status && phase === "needsReauth") {
    badge = <span className="badge t-amber">Needs sign-in</span>;
    body = (
      <div className="sync-phase">
        <span>{account.provider === "gmail" ? "Google" : service} signed this account out.</span>
        <span className="grow" />
        <button className="btn btn-secondary btn-sm" onClick={() => openReconnect(account.id)}>
          Reconnect <Icon name="external" size="xs" />
        </button>
      </div>
    );
  } else if (status && phase === "error") {
    badge = <span className="badge t-red">Sync error</span>;
    body = (
      <div className="sync-phase">
        <span title={status.error ?? undefined}>{syncFixFor(status)?.message ?? "Sync stopped with an error."}</span>
        <span className="grow" />
        <SyncFixButton status={status} />
      </div>
    );
  }

  return (
    <article className={`sync-account t-${tone}`}>
      <i className="dot" />
      <div>
        <div className="sync-name">
          <strong className="truncate">{name}</strong>
          {name !== account.email && <span className="sync-email truncate">{account.email}</span>}
          <span className="grow" />
          {badge}
        </div>
        {body}
      </div>
    </article>
  );
}

export function GoogleG() {
  return (
    <svg className="s2-google" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M21.6 12.23c0-.71-.06-1.39-.18-2.05H12v3.88h5.38a4.61 4.61 0 0 1-2 3.03v2.52h3.24c1.9-1.75 2.98-4.33 2.98-7.38ZM12 22c2.7 0 4.96-.9 6.62-2.39l-3.24-2.52c-.9.6-2.05.96-3.38.96-2.6 0-4.81-1.75-5.6-4.1H3.05v2.6A10 10 0 0 0 12 22ZM6.4 13.95a6 6 0 0 1 0-3.9V7.46H3.05a10 10 0 0 0 0 9.08l3.35-2.59ZM12 5.95c1.47 0 2.78.51 3.82 1.51l2.87-2.87A9.62 9.62 0 0 0 12 2a10 10 0 0 0-8.95 5.46l3.35 2.59c.79-2.35 3-4.1 5.6-4.1Z" />
    </svg>
  );
}
