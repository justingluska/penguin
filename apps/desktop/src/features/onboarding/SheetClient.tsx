// Optional step: a second, iOS-type Google client so sign-in happens in the
// macOS system sheet instead of a browser tab (docs/google-setup.md §5b).
import { useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { OAuthClientStatus } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { links } from "./links";
import { Callout, CopyButton, Field, OpenButton, Step, Steps, Troubleshooting, Ui } from "./parts";
import "./onboarding.css";

export const SHEET_NAMES = { client: "Penguin Mac (sheet)", bundleId: "co.gluska.penguin" };

export function SheetClientStep({ projectId, status, onStatus }: { projectId: string; status: OAuthClientStatus; onStatus: (s: OAuthClientStatus) => void }) {
  return (
    <>
      <Steps>
        <Step title="Open Create client" action={<OpenButton url={links.createClient(projectId)} primary>Open Clients</OpenButton>}>
          The same project as before.
        </Step>
        <Step title={<>Application type: <Ui>iOS</Ui></>}>
          <Field name="Name">
            <CopyButton text={SHEET_NAMES.client} label="client name" />
          </Field>
          <Field name="Bundle ID">
            <CopyButton text={SHEET_NAMES.bundleId} label="bundle ID" />
          </Field>
          Leave App Store ID and Team ID empty, then <Ui>Create</Ui>.
        </Step>
        <Step title="Paste the Client ID">
          It ends in <span className="mono">.apps.googleusercontent.com</span>. iOS clients have no secret, so there's nothing to download.
          <SheetClientField status={status} onStatus={onStatus} />
        </Step>
      </Steps>
      <Callout title="Skip this and Penguin signs in through your browser">
        Both work the same. The sheet just closes itself when you're done, so no tab is left behind.
      </Callout>
      <Troubleshooting
        problems={[
          {
            symptom: "A Workspace account is blocked in the sheet but works in the browser",
            fix: "Workspace admins approve apps per client ID. Approve this iOS client ID too, the same way as the Desktop one.",
          },
          {
            symptom: "“This client has a secret…”",
            fix: "That's the Desktop client's ID or JSON. Create a separate client with Application type iOS and paste its Client ID.",
          },
          {
            symptom: "The sheet doesn't open",
            fix: "Remove the iOS client here (or in Settings → Accounts). Penguin goes back to signing in through your browser.",
          },
        ]}
      />
    </>
  );
}

/** Paste / show / remove the iOS client ID. Also usable from Settings. */
export function SheetClientField({ status, onStatus }: { status: OAuthClientStatus; onStatus: (s: OAuthClientStatus) => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(p: Promise<OAuthClientStatus>) {
    setBusy(true);
    setError(null);
    try {
      onStatus(await p);
      setText("");
    } catch (e) {
      const m = asCommandError(e).message;
      setError(m.charAt(0).toUpperCase() + m.slice(1));
    } finally {
      setBusy(false);
    }
  }

  if (status.iosClientId) {
    return (
      <div className="onb-client-done onb-sheet-done" role="status">
        <span className="onb-client-ok">
          <Icon name="check" size="xs" />
        </span>
        <div className="grow min0">
          <div className="onb-client-title">Sign-in sheet on</div>
          <div className="mono truncate onb-client-id" title={status.iosClientId}>
            {status.iosClientId}
          </div>
        </div>
        <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => void run(api.clearIosOauthClient())}>
          Remove
        </button>
      </div>
    );
  }

  return (
    <div className="onb-sheet-field">
      <div className="onb-sheet-row">
        <label className="onb-input grow">
          <span className="onb-input-label">Client ID</span>
          <input
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && text.trim()) {
                e.preventDefault();
                void run(api.setIosOauthClient(text.trim()));
              }
            }}
            placeholder="….apps.googleusercontent.com, .plist or JSON"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-invalid={!!error}
          />
        </label>
        <button className="btn btn-secondary" disabled={busy || !text.trim()} onClick={() => void run(api.setIosOauthClient(text.trim()))}>
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
      {error && (
        <div className="onb-error" role="alert">
          <Icon name="info" size="xs" />
          <span>{error}</span>
        </div>
      )}
    </div>
  );
}

/** Settings copy: shown next to the field once accounts exist. */
export const RECONNECT_NOTE = "Existing accounts keep using the browser client until you reconnect them.";

/** "System sheet (iOS client …xyz)" or "Browser". */
export function signInMethodLabel(status: OAuthClientStatus): string {
  const id = status.iosClientId;
  if (!id) return "Browser";
  return `System sheet (iOS client …${id.replace(/\.apps\.googleusercontent\.com$/, "").slice(-6)})`;
}

