// Top-of-message banner in the thread view: the code, big, with a Copy
// button (⌘C while the banner has focus, ⌘⇧C from anywhere in the thread).
import { useEffect, useState } from "react";
import type { MessageView, ThreadRef } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { Keys } from "../../components/Kbd";
import { clearBannerCode, COPY_KEYS, copyCode, FRESH_MS, otpAge, setBannerCode, useNow } from "./otp";
import "./otp.css";

export function OtpBanner({ m }: { m: MessageView }) {
  const otp = m.otp;
  const code = otp?.kind === "code" ? otp.code : null;
  const now = useNow(code !== null);
  const [copied, setCopied] = useState(false);

  // ⌘⇧C copies the banner on screen; messages mount oldest first, so the
  // newest expanded code wins.
  useEffect(() => {
    if (!code) return;
    const entry = { ref: { accountId: m.accountId, threadId: m.threadId } satisfies ThreadRef, code };
    setBannerCode(entry);
    return () => clearBannerCode(entry);
  }, [code, m.accountId, m.threadId]);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);

  if (!otp) return null;
  if (otp.kind === "link") {
    return (
      <div className={"otp-banner is-link" + (otp.verified ? "" : " is-unverified")} role="note">
        <Icon name="link" size="sm" />
        <span className="otp-banner-label">One-time sign-in link below. Use it only if you asked to sign in.</span>
      </div>
    );
  }
  if (!code) return null;
  const stale = otpAge(otp, now) >= FRESH_MS;
  const copy = async () => {
    if (await copyCode(code)) setCopied(true);
  };
  return (
    <div
      className={"otp-banner" + (stale ? " is-stale" : "") + (otp.verified ? "" : " is-unverified")}
      tabIndex={0}
      role="group"
      aria-label={`Verification code ${code}`}
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "c" && !window.getSelection()?.toString()) {
          e.preventDefault();
          void copy();
        }
      }}
    >
      <div className="otp-banner-text">
        <span className="otp-banner-label">
          {otp.verified ? "Verification code" : "Verification code · unverified sender"}
          {stale && <span className="otp-hint">may have expired</span>}
        </span>
        <span className="otp-banner-code" onDoubleClick={() => void copy()}>
          {code}
        </span>
      </div>
      <button type="button" className={"btn btn-sm otp-copy" + (copied ? " is-copied" : "")} onClick={() => void copy()}>
        <Icon name={copied ? "check" : "copy"} size="xs" />
        {copied ? "Copied" : "Copy"}
        {!copied && <Keys keys={COPY_KEYS} />}
      </button>
    </div>
  );
}
