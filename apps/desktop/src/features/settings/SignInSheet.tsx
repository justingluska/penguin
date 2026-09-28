// Settings → Accounts → "Sign-in method": the optional iOS-type Google client
// that makes sign-in use the macOS system sheet instead of a browser tab. The
// field itself is onboarding's SheetClientField. OWNER: settings agent.
import { useEffect, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { OAuthClientStatus } from "../../lib/types";
import { RECONNECT_NOTE, SheetClientField, SHEET_NAMES, signInMethodLabel } from "../onboarding/SheetClient";

export function SignInSheetRow() {
  const [status, setStatus] = useState<OAuthClientStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.oauthClientStatus().then(
      (s) => live && setStatus(s),
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, []);

  return (
    <div className="setting-row setting-tall st-sheet">
      <div className="min0 grow">
        <span className="setting-label">
          Sign-in method{status ? <span className="st-sheet-value">: {signInMethodLabel(status)}</span> : null}
        </span>
        <p className="st-muted">
          {status?.iosClientId
            ? "Google sign-in opens in the macOS sheet."
            : `Optional: with an iOS Google client (bundle ID ${SHEET_NAMES.bundleId}), sign-in uses the macOS sheet instead of a browser tab.`}
        </p>
        {error ? <p className="st-error">{error}</p> : null}
        {status ? <SheetClientField status={status} onStatus={setStatus} /> : null}
        <p className="st-muted st-sheet-note">{RECONNECT_NOTE}</p>
      </div>
    </div>
  );
}
