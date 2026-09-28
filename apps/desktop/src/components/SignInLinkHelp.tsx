// "Browser didn't open? Open it again · Copy link" under every screen that
// waits for a browser sign-in (Add account, Reconnect). Mount it only while
// waiting: it listens for penguin://sign-in-url, and asks sign_in_link once
// in case the link was published before it mounted. Shows nothing for the
// macOS sign-in sheet, which has no link.
import { useEffect, useState } from "react";
import { api, asCommandError, onSignInUrl } from "../lib/api";
import type { SignInLink } from "../lib/types";
import { copyText } from "../lib/clipboard";
import { Icon } from "./Icon";
import "./signInLinkHelp.css";

export function SignInLinkHelp() {
  const [link, setLink] = useState<SignInLink | null>(null);
  const [reopenError, setReopenError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    const un = onSignInUrl((l) => {
      if (!live) return;
      setLink(l);
      setReopenError(null);
    });
    // After the listener is attached, so nothing published in between is lost.
    void un.then(() =>
      api.signInLink().then(
        (l) => live && l && setLink((cur) => cur ?? l),
        () => {},
      ),
    );
    return () => {
      live = false;
      void un.then((f) => f());
    };
  }, []);

  if (!link) return null;

  async function reopen() {
    setBusy(true);
    setReopenError(null);
    try {
      await api.reopenSignIn();
    } catch (e) {
      setReopenError(asCommandError(e).message);
    } finally {
      setBusy(false);
    }
  }

  const error = reopenError ?? (link.error ? `Penguin couldn't open your browser (${link.error}).` : null);
  return (
    <div className="signin-help">
      {error && (
        <span className="signin-help-error" role="alert">
          <Icon name="info" size="xs" />
          {error}
        </span>
      )}
      <span className="signin-help-row">
        {!error && <span>Browser didn't open?</span>}
        <button type="button" className="signin-help-btn" onClick={() => void reopen()} disabled={busy}>
          {error ? "Try again" : "Open it again"}
        </button>
        <span aria-hidden="true">·</span>
        <button type="button" className="signin-help-btn" onClick={() => void copyText(link.url, "Sign-in link copied")}>
          Copy link
        </button>
        {error && <span>and paste it into your browser</span>}
      </span>
    </div>
  );
}
