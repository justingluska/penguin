// Add account screens that aren't the Google Cloud guide: the email field and
// provider picker, the Microsoft (Entra) registration, app-password
// instructions and Connect, server settings, "not supported yet", and the
// "coming in the next update" card. Steps are assembled in flow.tsx.
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { api, asCommandError, onSyncStatus } from "../../lib/api";
import type { Account, DetectedProvider, MailSecurity, MicrosoftClientStatus, PendingSetup, ServerSettings, SetupKind, SyncStatus } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { accountName } from "../../components/Identity";
import { GoogleG, SyncRow } from "./Accounts";
import { notePendingError, providerName } from "./pending";
import { Callout, CopyButton, Field, OpenButton, Step, Steps, Troubleshooting, Ui } from "./parts";
// Its marks and steps are drawn by onboarding.css wherever they appear (Settings too).
import "./onboarding.css";
import {
  CHOICES,
  GMAIL_QUICK,
  GMAIL_QUICK_DISCLOSURE,
  MICROSOFT,
  WORKSPACE_NO_APP_PASSWORDS,
  gmailQuickSetupRequest,
  quickSetupAllowed,
  adminConsentUrl,
  cleanAppPassword,
  connectErrorText,
  defaultPort,
  initialServerForm,
  linkParts,
  looksLikeEmail,
  microsoftErrorText,
  normalizeClientId,
  passwordGuide,
  serverFormErrors,
  type MicrosoftError,
  type ServerForm,
} from "./providers";

// ---------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------

/** Neutral provider marks: letters and generic icons (Google's G is already in the app). */
export function ProviderMark({ setup }: { setup: SetupKind }) {
  const letter: Partial<Record<SetupKind, string>> = { microsoft: "M", yahoo: "Y", aol: "A", fastmail: "F", proton: "P" };
  return (
    <span className="onb-mark" aria-hidden="true">
      {setup === "google" ? (
        <GoogleG />
      ) : setup === "icloud" ? (
        <Icon name="cloud" size="sm" />
      ) : setup === "imap" ? (
        <Icon name="mail" size="sm" />
      ) : setup === "unsupported" ? (
        <Icon name="globe" size="sm" />
      ) : (
        letter[setup]
      )}
    </span>
  );
}

/** Step text: [[Label]] becomes a UI chip, {email} the address. */
export function rich(text: string, email: string): ReactNode {
  return text.split(/(\[\[.+?\]\]|\{email\})/).map((part, i) =>
    part === "{email}" ? (
      <b key={i} className="onb-inline-email">
        {email}
      </b>
    ) : part.startsWith("[[") ? (
      <Ui key={i}>{part.slice(2, -2)}</Ui>
    ) : (
      part
    ),
  );
}

function openLink(e: React.MouseEvent<HTMLAnchorElement>) {
  e.preventDefault();
  void api.openExternal(e.currentTarget.href);
}

/** Live sync status by account, for the "Connected" card. */
function useSync(): Record<string, SyncStatus> {
  const [sync, setSync] = useState<Record<string, SyncStatus>>({});
  useEffect(() => {
    let live = true;
    api.syncStatus().then((st) => live && setSync((m) => ({ ...Object.fromEntries(st.map((s) => [s.accountId, s])), ...m })), () => {});
    const un = onSyncStatus((s) => setSync((m) => ({ ...m, [s.accountId]: s })));
    return () => {
      live = false;
      void un.then((f) => f());
    };
  }, []);
  return sync;
}

function Connected({ account, allAccounts, provider }: { account: Account; allAccounts: Account[]; provider: string }) {
  const sync = useSync();
  return (
    <section className="card onb-synclist" aria-label="Connected account">
      <SyncRow account={account} name={accountName(account, allAccounts)} status={sync[account.id]} provider={provider} />
    </section>
  );
}

export function addedAccount(detected: DetectedProvider, accounts: Account[]): Account | undefined {
  const e = detected.email.toLowerCase();
  return accounts.find((a) => a.email.toLowerCase() === e);
}

// ---------------------------------------------------------------------------
// Email + picker
// ---------------------------------------------------------------------------

export function EmailForm({
  email,
  setEmail,
  detecting,
  error,
  onSubmit,
  onChoose,
  chosen = null,
  pending = [],
  onResume,
  autoFocus,
}: {
  email: string;
  setEmail: (s: string) => void;
  /** The domain being checked, or null. */
  detecting: string | null;
  error: string | null;
  onSubmit: () => void;
  /** A provider button: detects with it right away once the address is typed; before that, marks it chosen. */
  onChoose: (setup: SetupKind) => void;
  /** Provider chosen before the address was typed (used when it's submitted). */
  chosen?: SetupKind | null;
  /** Unfinished setups, offered as "Continue setting up …". */
  pending?: PendingSetup[];
  onResume?: (p: PendingSetup) => void;
  autoFocus?: boolean;
}) {
  const input = useRef<HTMLInputElement>(null);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit();
  };
  const choose = (s: SetupKind) => {
    onChoose(s);
    if (!looksLikeEmail(email)) input.current?.focus();
  };
  return (
    <div className="onb-emailstep">
      {pending.length > 0 && onResume && (
        <div className="onb-resume-list">
          {pending.map((p) => (
            <button key={p.email} type="button" className="onb-resume" onClick={() => onResume(p)} disabled={!!detecting}>
              <ProviderMark setup={p.kind} />
              <span className="onb-resume-text">
                <b className="truncate">Continue setting up {p.email}</b>
                <span className={p.lastError ? "onb-resume-error truncate" : "truncate"}>{p.lastError ?? providerName(p.kind)}</span>
              </span>
              <Icon name="right" size="xs" />
            </button>
          ))}
        </div>
      )}
      <form className="onb-emailform" onSubmit={submit} noValidate>
        <div className={`onb-emailfield${error ? " is-invalid" : ""}`}>
          <input
            ref={input}
            id="onb-email"
            type="email"
            aria-label="Email address"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="you@example.com"
            autoComplete="email"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoFocus={autoFocus}
            aria-invalid={!!error}
            aria-describedby="onb-email-status"
          />
          {detecting && <span className="onb-spin" aria-hidden="true" />}
        </div>
        <div id="onb-email-status" className="onb-emailfield-status" role="status">
          {error ? (
            <span className="onb-field-error">
              <Icon name="info" size="xs" />
              {error}
            </span>
          ) : detecting ? (
            <span className="s2-muted">Checking {detecting}…</span>
          ) : null}
        </div>
      </form>
      <ProviderPicker disabled={!!detecting} onChoose={choose} chosen={chosen} />
    </div>
  );
}

export function ProviderPicker({
  disabled,
  onChoose,
  exclude,
  chosen,
}: {
  disabled?: boolean;
  onChoose: (s: SetupKind) => void;
  exclude?: SetupKind;
  chosen?: SetupKind | null;
}) {
  return (
    <div className="onb-pick" role="group" aria-label="Choose your provider">
      {CHOICES.filter((c) => c.setup !== exclude).map((c) => (
        <button
          key={c.setup}
          type="button"
          className={`onb-pick-btn${chosen === c.setup ? " is-chosen" : ""}`}
          disabled={disabled}
          aria-pressed={chosen ? chosen === c.setup : undefined}
          onClick={() => onChoose(c.setup)}
        >
          <ProviderMark setup={c.setup} />
          <span className="onb-pick-text">
            <b>{c.label}</b>
            <span>{c.sub}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

/** "sam@harbor.example [Yahoo Mail] Change" above every provider step. */
export function WhoChip({ detected, onChange }: { detected: DetectedProvider; onChange: () => void }) {
  return (
    <div className="onb-who">
      <ProviderMark setup={detected.setup} />
      <span className="onb-who-email truncate">{detected.email}</span>
      <span className="badge t-gray onb-who-tag">
        {detected.setup !== "unsupported" ? detected.displayName : detected.offline ? "Not checked" : "Not recognized"}
      </span>
      <span className="grow" />
      <button type="button" className="btn btn-ghost btn-sm" onClick={onChange}>
        Change
      </button>
    </div>
  );
}

/** The last step while this build can't connect the provider yet. Not an error. */
export function ComingSoon({ detected, onRestart, kept }: { detected: DetectedProvider; onRestart: () => void; kept?: ReactNode }) {
  return (
    <div className="onb-soon" role="status">
      <span className="onb-soon-icon">
        <Icon name="clock" size="sm" />
      </span>
      <div className="onb-soon-body">
        <div className="onb-soon-title">{detected.displayName} accounts are coming in the next update</div>
        {kept && <p>{kept}</p>}
        <div className="s2-action-row">
          <button type="button" className="btn btn-secondary btn-sm" onClick={onRestart}>
            Use a different address
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Not supported (yet)
// ---------------------------------------------------------------------------

export function unsupportedLead(d: DetectedProvider): string {
  if (d.offline) return "You seem to be offline. Try again, or choose your provider.";
  if (d.gateway) return `Mail passes through ${d.gateway}, which hides the provider. Choose it below if you know it.`;
  if (!d.mxHost) return `No mail servers found for ${d.domain}. Check for a typo, or choose your provider.`;
  return `Its mail is handled by ${d.mxHost}. Choose the provider, or set it up by hand.`;
}

export function UnsupportedStep({ detected, onChoose, onRetry }: { detected: DetectedProvider; onChoose: (s: SetupKind) => void; onRetry: () => void }) {
  return (
    <>
      {detected.offline && (
        <div className="s2-action-row">
          <button type="button" className="btn btn-secondary" onClick={onRetry}>
            <Icon name="refresh" size="sm" />
            Try again
          </button>
        </div>
      )}
      <div className="onb-pick-group">
        <div className="onb-pick-label">Hosted by one of these?</div>
        <ProviderPicker onChoose={onChoose} exclude="imap" />
      </div>
      <section className="onb-manual">
        <div className="onb-manual-head">
          <ProviderMark setup="imap" />
          <div>
            <b>Set it up by hand (IMAP)</b>
            <p>You'll need:</p>
          </div>
        </div>
        <ul className="onb-needs">
          <li>The incoming (IMAP) server and port, like imap.{detected.domain} and 993</li>
          <li>The outgoing (SMTP) server and port, like smtp.{detected.domain} and 465 or 587</li>
          <li>Your username and password, or an app password if the provider uses them</li>
        </ul>
        <div className="s2-action-row">
          <button type="button" className="btn btn-primary" onClick={() => onChoose("imap")}>
            Enter server settings
          </button>
        </div>
      </section>
    </>
  );
}

// ---------------------------------------------------------------------------
// Connecting (shared by Microsoft, app passwords and IMAP)
// ---------------------------------------------------------------------------

type Connect = { busy: boolean; error: string | null; msError: MicrosoftError | null; run: (fn: () => Promise<Account>) => Promise<void>; cancel: () => void };

/**
 * `email`: the unfinished setup a failure is noted on (Settings → Accounts
 * shows it). `unreachable`: what a network error says (the provider's words).
 */
function useConnect(
  onAdded: (a: Account) => void,
  email: string,
  microsoft = false,
  unreachable = "Couldn't reach the server. Check your connection and the server name and port, then try again.",
): Connect {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [msError, setMsError] = useState<MicrosoftError | null>(null);
  const attempt = useRef(0);
  const pending = useRef(false);
  // Leaving mid sign-in abandons it, so its loopback listener closes.
  useEffect(
    () => () => {
      if (pending.current && microsoft) void api.cancelSignIn().catch(() => {});
    },
    [microsoft],
  );
  return {
    busy,
    error,
    msError,
    run: async (fn) => {
      const id = ++attempt.current;
      setBusy(true);
      setError(null);
      setMsError(null);
      pending.current = true;
      try {
        const acc = await fn();
        if (id === attempt.current) onAdded(acc);
      } catch (e) {
        if (id !== attempt.current) return;
        const err = asCommandError(e);
        if (err.code === "cancelled") return;
        const text = microsoft ? microsoftErrorText(err.message).text : connectErrorText(err, unreachable);
        if (microsoft) setMsError(microsoftErrorText(err.message));
        else setError(text);
        notePendingError(email, text);
      } finally {
        if (id === attempt.current) {
          setBusy(false);
          pending.current = false;
        }
      }
    },
    cancel: () => {
      attempt.current++;
      pending.current = false;
      setBusy(false);
      if (microsoft) void api.cancelSignIn().catch(() => {});
    },
  };
}

/** An error line; web addresses in `text` become links that open in the browser. */
function ErrorLine({ children, text }: { children?: ReactNode; text?: string }) {
  return (
    <div className="onb-error" role="alert">
      <Icon name="info" size="xs" />
      <span className="grow">
        {children}
        {text !== undefined &&
          linkParts(text).map((p, i) =>
            p.url ? (
              <a key={i} className="onb-link" href={p.url} onClick={openLink}>
                {p.text}
              </a>
            ) : (
              p.text
            ),
          )}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Microsoft
// ---------------------------------------------------------------------------

export function MicrosoftRegisterStep() {
  return (
    <>
      <Callout title="What you need">
        A work or school account. With only Outlook.com or Hotmail, create a{" "}
        <a className="onb-link" href={MICROSOFT.azureFree} onClick={openLink}>
          free Azure account
        </a>{" "}
        first.
      </Callout>
      <Steps>
        <Step title="Open App registrations" action={<OpenButton url={MICROSOFT.appRegistrations} primary>Open Microsoft Entra</OpenButton>}>
          Sign in, then click <Ui>New registration</Ui>.
        </Step>
        <Step title="Name it">
          <Field name="Name">
            <CopyButton text={MICROSOFT.appName} label="app name" />
          </Field>
        </Step>
        <Step title="Supported account types: the widest option">
          <div className="onb-choice">
            <div className="onb-choice-card is-pick">
              <b>Any organization + personal accounts</b>
              <span>Works for Outlook.com and work accounts. Pick this.</span>
            </div>
            <div className="onb-choice-card">
              <b>This organization only</b>
              <span>Outlook.com and other companies can't sign in.</span>
            </div>
          </div>
        </Step>
        <Step title={<>Redirect URI: <Ui>Public client/native (mobile &amp; desktop)</Ui></>}>
          <Field name="Redirect URI">
            <CopyButton text={MICROSOFT.redirectUri} label="redirect URI" />
          </Field>
          Then click <Ui>Register</Ui>.
        </Step>
      </Steps>
      <Troubleshooting
        problems={[
          {
            symptom: "“You don't have access” or no App registrations",
            fix: "Your account has no directory, or your organization stops users from registering apps. Create a free Azure account for a directory of your own, or ask your IT admin to register it.",
          },
          {
            symptom: "Chose the Web platform for the redirect",
            fix: "Open Authentication, delete the Web redirect, then Add a platform → Mobile and desktop applications → http://localhost.",
          },
          {
            symptom: "“Allow public client flows”",
            fix: "Leave it at No. Penguin uses Microsoft's standard browser sign-in with PKCE, which doesn't need it.",
          },
        ]}
      />
    </>
  );
}

export function MicrosoftPermissionsStep({ detected }: { detected: DetectedProvider }) {
  return (
    <>
      <Steps>
        <Step title={<>In your app, open <Ui>API permissions</Ui></>}>
          Click <Ui>Add a permission</Ui> → <Ui>Microsoft Graph</Ui> → <Ui>Delegated permissions</Ui>.
        </Step>
        <Step title="Tick these five">
          Search for each one. <Ui>User.Read</Ui> is usually there already.
          <div className="onb-perms">
            {MICROSOFT.permissions.map((p) => (
              <CopyButton key={p} text={p} label={`permission ${p}`} />
            ))}
          </div>
        </Step>
        <Step title={<>Click <Ui>Add permissions</Ui></>} />
      </Steps>
      {detected.kind === "microsoft365" ? (
        <Callout title="Admin of your organization?">
          Also click <Ui>Grant admin consent</Ui>, so nobody sees "Need admin approval". Not an admin? Skip it.
        </Callout>
      ) : (
        <Callout tone="ok">
          Personal accounts approve this themselves at sign-in: skip <Ui>Grant admin consent</Ui>.
        </Callout>
      )}
    </>
  );
}

export function MicrosoftClientStep({ ms, setMs }: { ms: MicrosoftClientStatus | null; setMs: (s: MicrosoftClientStatus) => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(!ms?.clientId);
  const parsed = normalizeClientId(text);

  async function save(e?: FormEvent) {
    e?.preventDefault();
    if (!parsed) {
      setError("That isn't an Application (client) ID. It looks like 1b2c3d4e-0000-1111-2222-333344445555.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setMs(await api.setMicrosoftClient(parsed));
      setEditing(false);
      setText("");
    } catch (err) {
      setError(asCommandError(err).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Steps>
        <Step title={<>Open the app's <Ui>Overview</Ui></>}>
          Under <Ui>Essentials</Ui>, copy <Ui>Application (client) ID</Ui>. Not the Directory (tenant) ID below it.
        </Step>
        <Step title="Paste it here">
          {ms?.clientId && !editing ? (
            <div className="onb-client-done onb-sheet-done">
              <span className="onb-client-ok">
                <Icon name="check" size="sm" />
              </span>
              <div className="grow">
                <div className="onb-client-title">Microsoft client added</div>
                <div className="onb-client-id mono">{ms.clientId}</div>
                <div className="onb-client-path mono">{ms.path}</div>
              </div>
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEditing(true)}>
                Replace
              </button>
            </div>
          ) : (
            <form className="onb-sheet-field" onSubmit={(e) => void save(e)}>
              <div className="onb-sheet-row">
                <label className={`onb-input${error ? " is-invalid" : ""}`}>
                  <span className="onb-input-label">Client ID</span>
                  <input
                    value={text}
                    onChange={(e) => {
                      setText(e.target.value);
                      setError(null);
                    }}
                    placeholder="1b2c3d4e-0000-1111-2222-333344445555"
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    autoFocus
                    aria-invalid={!!error}
                  />
                  {parsed && <Icon name="check" size="xs" className="onb-input-ok" />}
                </label>
                <button type="submit" className="btn btn-secondary" disabled={!text.trim() || busy}>
                  Save
                </button>
              </div>
              {error && <ErrorLine>{error}</ErrorLine>}
            </form>
          )}
        </Step>
      </Steps>
    </>
  );
}

export function MicrosoftSignInStep({
  detected,
  ms,
  accounts,
  allAccounts,
  onAdded,
  onRestart,
  onEditClient,
}: {
  detected: DetectedProvider;
  ms: MicrosoftClientStatus | null;
  accounts: Account[];
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  onRestart: () => void;
  onEditClient: () => void;
}) {
  const c = useConnect(onAdded, detected.email, true);
  const added = addedAccount(detected, accounts);
  const clientId = ms?.clientId ?? null;
  const consentUrl = clientId ? adminConsentUrl(detected.domain, clientId) : null;

  if (!detected.available) {
    return <ComingSoon detected={detected} onRestart={onRestart} kept={clientId ? "Your client ID is saved, so you won't need to paste it again." : undefined} />;
  }
  return (
    <>
      {added ? (
        <Connected account={added} allAccounts={allAccounts} provider="Microsoft" />
      ) : (
        <div className="onb-signin">
          {c.busy ? (
            <>
              <span className="onb-wait" role="status">
                <span className="onb-spin" aria-hidden="true" />
                Finish signing in in your browser…
              </span>
              <span className="grow" />
              <button type="button" className="btn btn-secondary btn-sm" onClick={c.cancel}>
                Cancel
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                className="btn btn-primary onb-google-btn"
                autoFocus
                disabled={!clientId}
                onClick={() => void c.run(() => api.connectAccount({ email: detected.email, kind: detected.kind, auth: "microsoftOAuth", clientId }))}
              >
                <ProviderMark setup="microsoft" />
                Sign in with Microsoft
              </button>
            </>
          )}
        </div>
      )}
      {c.msError && !c.msError.adminConsent && <ErrorLine>Sign-in didn't finish. {c.msError.text}</ErrorLine>}
      {!added && consentUrl && (c.msError?.adminConsent || detected.kind === "microsoft365") && (
        <Callout tone={c.msError?.adminConsent ? "warn" : "info"} title={c.msError?.adminConsent ? "Your IT admin needs to approve Penguin" : "Work or school account?"}>
          {c.msError?.adminConsent
            ? "Send your IT admin this link, then sign in again once they approve it."
            : "If Microsoft says Need admin approval, send your IT admin this link:"}
          <div className="onb-consent">
            <CopyButton text={consentUrl} label="admin approval link" />
          </div>
        </Callout>
      )}
      {clientId && !added && (
        <p className="s2-muted">
          Using client <span className="mono">{clientId}</span>.{" "}
          <button type="button" className="onb-textbtn" onClick={onEditClient}>
            Change
          </button>
        </p>
      )}
      <Troubleshooting
        problems={[
          { symptom: "“Need admin approval”", fix: "Your organization requires an admin to approve apps that read mail. Send them the approval link above, then sign in again." },
          { symptom: "AADSTS700016: application not found", fix: "The client ID is wrong, or it's the Directory (tenant) ID. Copy Application (client) ID from the app's Overview." },
          { symptom: "“unauthorized_client” with an Outlook.com account", fix: "The app only allows work accounts. In Authentication → Supported accounts, include personal Microsoft accounts." },
          { symptom: "AADSTS50011: redirect URI mismatch", fix: "Add http://localhost under Mobile and desktop applications in Authentication." },
        ]}
      />
    </>
  );
}

// ---------------------------------------------------------------------------
// App passwords: Yahoo, AOL, iCloud, Fastmail; and Proton Bridge's guide
// ---------------------------------------------------------------------------

/**
 * Gmail quick setup as the second option under Google: a card with the
 * button, or for a Workspace address, why it isn't offered.
 */
export function QuickSetupOffer({ detected, onChoose, compact }: { detected: DetectedProvider; onChoose: () => void; compact?: boolean }) {
  if (!quickSetupAllowed(detected)) {
    if (compact || detected.kind !== "googleWorkspace") return null;
    return (
      <Callout title="Why there's no quick setup for this account">
        {WORKSPACE_NO_APP_PASSWORDS}
      </Callout>
    );
  }
  if (compact) {
    return (
      <p className="onb-shortcut">
        Rather not use a Google Cloud client?{" "}
        <button type="button" className="onb-textbtn" onClick={onChoose}>
          Gmail quick setup with an app password <Icon name="right" size="2xs" />
        </button>
      </p>
    );
  }
  return (
    <section className="onb-manual" aria-label="Gmail quick setup">
      <div className="onb-manual-head">
        <ProviderMark setup="google" />
        <div>
          <b>Gmail – quick setup (no Google Cloud project)</b>
          <p>Connect with an app password instead, in about 2 minutes. Needs 2-Step Verification on your Google Account.</p>
        </div>
      </div>
      <p className="s2-muted">{GMAIL_QUICK_DISCLOSURE}</p>
      <div className="s2-action-row">
        <button type="button" className="btn btn-secondary" onClick={onChoose}>
          Use quick setup
        </button>
      </div>
    </section>
  );
}

/** The way back from quick setup to the recommended path. */
function BackToCloud({ onBack }: { onBack?: () => void }) {
  if (!onBack) return null;
  return (
    <p className="onb-shortcut">
      Want faster sync and access you can limit?{" "}
      <button type="button" className="onb-textbtn" onClick={onBack}>
        Use your own Google Cloud client instead <Icon name="right" size="2xs" />
      </button>
    </p>
  );
}

export function PasswordGuideStep({ detected, quick, onBack }: { detected: DetectedProvider; quick?: boolean; onBack?: () => void }) {
  const g = quick ? GMAIL_QUICK : passwordGuide(detected.setup);
  if (!g) return null;
  if (quick && !quickSetupAllowed(detected)) return <WorkspaceNoQuickSetup onBack={onBack} />;
  return (
    <>
      {g.needs && <Callout title="What you need">{g.needs}</Callout>}
      <Steps>
        {g.steps.map((s, i) => (
          <Step
            key={i}
            title={rich(s.title, detected.email)}
            action={i === 0 ? <OpenButton url={g.page.url} primary>{g.page.label}</OpenButton> : undefined}
          >
            {s.detail ? rich(s.detail, detected.email) : undefined}
          </Step>
        ))}
      </Steps>
      {g.note && <Callout>{g.note}</Callout>}
      {quick && (
        <Troubleshooting
          problems={[
            {
              symptom: "“The setting you are looking for is not available for your account”",
              fix: "Turn on 2-Step Verification (Google Account → Security → 2-Step Verification), then open App passwords again. Accounts with Advanced Protection, and Google Workspace accounts, can't create app passwords.",
            },
            { symptom: "Lost the password before pasting it", fix: "Delete it on the App passwords page and create a new one. Each is shown only once." },
          ]}
        />
      )}
      {quick && <BackToCloud onBack={onBack} />}
    </>
  );
}

function WorkspaceNoQuickSetup({ onBack }: { onBack?: () => void }) {
  return (
    <>
      <Callout tone="warn" title="Quick setup is for personal Gmail only">
        {WORKSPACE_NO_APP_PASSWORDS}
      </Callout>
      {onBack && (
        <div className="s2-action-row">
          <button type="button" className="btn btn-primary" onClick={onBack}>
            Sign in with Google instead
          </button>
        </div>
      )}
    </>
  );
}

export function PasswordConnectStep({
  detected,
  accounts,
  allAccounts,
  onAdded,
  onRestart,
  quick,
  onBack,
}: {
  detected: DetectedProvider;
  accounts: Account[];
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  onRestart: () => void;
  /** Gmail quick setup (an app password over IMAP) instead of the detected provider's own. */
  quick?: boolean;
  /** Quick setup: back to the Google Cloud path. */
  onBack?: () => void;
}) {
  const g = quick ? GMAIL_QUICK : passwordGuide(detected.setup);
  const [password, setPassword] = useState("");
  const service = quick ? "Gmail" : (g?.name ?? detected.displayName);
  const c = useConnect(onAdded, detected.email, false, `Couldn't reach ${service}. Check your internet connection, then try again.`);
  const added = addedAccount(detected, accounts);
  if (!g) return null;
  if (quick && !quickSetupAllowed(detected)) return <WorkspaceNoQuickSetup onBack={onBack} />;
  // Quick setup rides IMAP, which this build connects; `available` is about Google sign-in.
  if (!quick && !detected.available) return <ComingSoon detected={detected} onRestart={onRestart} />;
  if (added) return <Connected account={added} allAccounts={allAccounts} provider={quick ? "Gmail" : detected.displayName} />;

  const request = (pw: string) =>
    quick
      ? gmailQuickSetupRequest(detected.email, pw)
      : { email: detected.email, kind: detected.kind, auth: detected.auth, password: pw, imap: detected.imap, smtp: detected.smtp };
  const servers = quick ? gmailQuickSetupRequest(detected.email, "") : { imap: detected.imap, smtp: detected.smtp };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const pw = cleanAppPassword(password);
    if (!pw) return;
    void c.run(() => api.connectAccount(request(pw)));
  };
  return (
    <>
      <form className="onb-connect" onSubmit={submit}>
        <label className="onb-connect-row">
          <span className="onb-connect-label">{g.field}</span>
          <span className="onb-input onb-input-wide">
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={g.placeholder}
              autoComplete="off"
              spellCheck={false}
              autoFocus
            />
          </span>
        </label>
        <div className="onb-connect-row">
          <span className="onb-connect-label" />
          <span className="s2-action-row">
            <button type="submit" className="btn btn-primary" disabled={!cleanAppPassword(password) || c.busy}>
              {c.busy && <span className="onb-spin" aria-hidden="true" />}
              {c.busy ? "Connecting…" : "Connect"}
            </button>
            <span className="s2-muted">Stored in your Mac's Keychain.</span>
          </span>
        </div>
        {quick && (
          <div className="onb-connect-row">
            <span className="onb-connect-label" />
            <p className="s2-muted onb-disclosure">
              <Icon name="shield" size="xs" />
              {GMAIL_QUICK_DISCLOSURE}
            </p>
          </div>
        )}
      </form>
      {c.error && <ErrorLine text={`Couldn't connect. ${c.error}`} />}
      <ServerSummary imap={servers.imap ?? null} smtp={servers.smtp ?? null} />
      {quick && <BackToCloud onBack={onBack} />}
    </>
  );
}

const SECURITY_LABEL: Record<MailSecurity, string> = { tls: "SSL/TLS", starttls: "STARTTLS", plain: "None" };

function ServerSummary({ imap, smtp }: { imap: ServerSettings | null; smtp: ServerSettings | null }) {
  if (!imap && !smtp) return null;
  const line = (s: ServerSettings) => `${s.host}:${s.port} · ${SECURITY_LABEL[s.security]} · ${s.username}`;
  return (
    <details className="onb-trouble">
      <summary>
        <Icon name="right" size="xs" className="onb-trouble-chev" />
        Server settings
      </summary>
      <dl className="onb-servers">
        {imap && (
          <div>
            <dt>Incoming (IMAP)</dt>
            <dd className="mono">{line(imap)}</dd>
          </div>
        )}
        {smtp && (
          <div>
            <dt>Outgoing (SMTP)</dt>
            <dd className="mono">{line(smtp)}</dd>
          </div>
        )}
      </dl>
    </details>
  );
}

// ---------------------------------------------------------------------------
// Server settings (Other IMAP, Proton Bridge)
// ---------------------------------------------------------------------------

export function ServerSettingsStep({
  detected,
  accounts,
  allAccounts,
  onAdded,
  onRestart,
}: {
  detected: DetectedProvider;
  accounts: Account[];
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  onRestart: () => void;
}) {
  const [form, setForm] = useState<ServerForm>(() => initialServerForm(detected));
  const [tried, setTried] = useState(false);
  const c = useConnect(
    onAdded,
    detected.email,
    false,
    detected.setup === "proton"
      ? "Couldn't reach Proton Mail Bridge on this Mac. Make sure Bridge is open and signed in, and that the ports match its mail settings."
      : undefined,
  );
  const added = addedAccount(detected, accounts);
  const guide = passwordGuide(detected.setup);
  if (!detected.available) return <ComingSoon detected={detected} onRestart={onRestart} />;
  if (added) return <Connected account={added} allAccounts={allAccounts} provider={detected.displayName} />;

  const errors = serverFormErrors(form);
  const show = (k: keyof typeof errors) => (tried ? errors[k] : undefined);
  const setServer = (kind: "imap" | "smtp", patch: Partial<ServerSettings>) =>
    setForm((f) => {
      const cur = f[kind];
      const next = { ...cur, ...patch };
      // Switching security moves a default port along with it.
      if (patch.security && cur.port === defaultPort(kind, cur.security)) next.port = defaultPort(kind, patch.security);
      // One username for both, unless the user changes SMTP's separately.
      const other = kind === "imap" && patch.username !== undefined && f.smtp.username === cur.username ? { smtp: { ...f.smtp, username: patch.username } } : {};
      return { ...f, [kind]: next, ...other };
    });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setTried(true);
    if (Object.keys(errors).length) return;
    const trim = (s: ServerSettings) => ({ ...s, host: s.host.trim().toLowerCase(), username: s.username.trim() });
    void c.run(() =>
      api.connectAccount({ email: detected.email, kind: detected.kind, auth: detected.auth, password: form.password, imap: trim(form.imap), smtp: trim(form.smtp) }),
    );
  };

  const server = (kind: "imap" | "smtp", title: string, hostErr?: string, portErr?: string) => (
    <fieldset className="onb-server">
      <legend>{title}</legend>
      <div className="onb-server-row">
        <label className={`onb-input onb-input-wide${hostErr ? " is-invalid" : ""}`}>
          <span className="onb-input-label">Server</span>
          <input
            value={form[kind].host}
            onChange={(e) => setServer(kind, { host: e.target.value })}
            placeholder={`${kind}.${detected.domain}`}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-invalid={!!hostErr}
          />
        </label>
        <label className={`onb-input onb-input-port${portErr ? " is-invalid" : ""}`}>
          <span className="onb-input-label">Port</span>
          <input inputMode="numeric" value={String(form[kind].port || "")} onChange={(e) => setServer(kind, { port: Number(e.target.value.replace(/\D/g, "")) })} aria-invalid={!!portErr} />
        </label>
        <select
          className="onb-select"
          aria-label={`${title} security`}
          value={form[kind].security}
          onChange={(e) => setServer(kind, { security: e.target.value as MailSecurity })}
        >
          <option value="tls">SSL/TLS</option>
          <option value="starttls">STARTTLS</option>
          {form[kind].security === "plain" && <option value="plain">None</option>}
        </select>
      </div>
      {(hostErr || portErr) && <span className="onb-field-error">{hostErr ?? portErr}</span>}
    </fieldset>
  );

  return (
    <>
      <form className="onb-serverform" onSubmit={submit} noValidate>
        {server("imap", "Incoming mail (IMAP)", show("imapHost"), show("imapPort"))}
        {server("smtp", "Outgoing mail (SMTP)", show("smtpHost"), show("smtpPort"))}
        <fieldset className="onb-server">
          <legend>Sign-in</legend>
          <div className="onb-server-row">
            <label className={`onb-input onb-input-wide${show("username") ? " is-invalid" : ""}`}>
              <span className="onb-input-label">Username</span>
              <input value={form.imap.username} onChange={(e) => setServer("imap", { username: e.target.value })} spellCheck={false} autoCapitalize="off" autoCorrect="off" />
            </label>
          </div>
          <div className="onb-server-row">
            <label className={`onb-input onb-input-wide${show("password") ? " is-invalid" : ""}`}>
              <span className="onb-input-label">{guide?.field ?? "Password"}</span>
              <input type="password" value={form.password} onChange={(e) => setForm((f) => ({ ...f, password: e.target.value }))} autoComplete="off" placeholder={guide?.placeholder} />
            </label>
          </div>
          {(show("username") || show("password")) && <span className="onb-field-error">{show("username") ?? show("password")}</span>}
        </fieldset>
        <div className="s2-action-row">
          <button type="submit" className="btn btn-primary" disabled={c.busy}>
            {c.busy && <span className="onb-spin" aria-hidden="true" />}
            {c.busy ? "Connecting…" : "Connect"}
          </button>
          <span className="s2-muted">Stored in your Mac's Keychain.</span>
        </div>
      </form>
      {c.error && <ErrorLine text={`Couldn't connect. ${c.error}`} />}
    </>
  );
}
