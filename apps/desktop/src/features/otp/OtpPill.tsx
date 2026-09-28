// The verification code in a list row, in place of the snippet: a code chip
// with grouped monospace digits ("918 273") and a copy segment. The whole
// chip is one button: click (or Enter/Space when focused) copies the raw code
// without selecting or opening the row, and the copy segment turns into
// "Copied ✓" for a moment. Fresh codes from authenticated senders use the
// accent; after 20 min the chip goes neutral with "expired?"; unverified
// senders get a dashed chip with a shield. Magic sign-in links get the same
// shape with a link icon and nothing to copy.
import { useEffect, useState } from "react";
import type { Otp } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { useKeyTip } from "../../lib/shortcutHints";
import { formatKeys } from "../../lib/keyboard";
import { COPY_KEYS, copyCode, FRESH_MS, groupCode, otpAge, useNow } from "./otp";
import "./otp.css";

export function OtpPill({ otp }: { otp: Otp }) {
  const now = useNow();
  const tip = useKeyTip();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);

  if (otp.kind === "link") {
    return (
      <span className={"otp-chip is-link" + (otp.verified ? "" : " is-unverified")} title="One-time sign-in link: open the email to use it">
        <span className="otp-chip-main">
          <Icon name="link" size="xs" />
          <span className="otp-chip-text">Sign-in link</span>
        </span>
      </span>
    );
  }
  const code = otp.code!;
  const stale = otpAge(otp, now) >= FRESH_MS;
  const title = !otp.verified
    ? "Unverified sender: make sure you asked for this code. Click to copy."
    : stale
      ? "Sent over 20 minutes ago, so it may have expired. Click to copy."
      : tip("Copy code", formatKeys(COPY_KEYS).flat().join(""));
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();
  return (
    <button
      type="button"
      className={"otp-chip" + (stale ? " is-stale" : "") + (otp.verified ? "" : " is-unverified") + (copied ? " is-copied" : "")}
      title={title}
      aria-label={copied ? `Copied ${code}` : `Copy code ${code}`}
      onMouseDown={stop}
      onDoubleClick={stop}
      // Enter/Space copy (native button click) instead of opening the row.
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") e.stopPropagation();
      }}
      onClick={async (e) => {
        e.stopPropagation();
        if (await copyCode(code)) setCopied(true);
      }}
    >
      <span className="otp-chip-main">
        {!otp.verified && <Icon name="shield" size="xs" className="otp-chip-shield" />}
        <span className="otp-chip-code">
          {groupCode(code).map((g, i) => (
            <span key={i}>{g}</span>
          ))}
        </span>
        {stale && !copied && <span className="otp-chip-hint">expired?</span>}
      </span>
      <span className="otp-chip-copy" aria-hidden="true">
        <Icon name={copied ? "check" : "copy"} size="xs" />
        {copied && <span>Copied</span>}
      </span>
    </button>
  );
}
