// Whether the app opens on first-run setup instead of mail (App.tsx boot).
// Setup is for "nothing to show yet": no accounts at all, or Gmail accounts
// whose Google OAuth client is missing (they can't sync until it's added
// back). An IMAP- or Microsoft-only user never needs a Google client, so
// they go straight to mail.
import type { Account, OAuthClientStatus } from "../lib/types";

export function needsSetup(status: Pick<OAuthClientStatus, "configured">, accounts: Pick<Account, "provider">[]): boolean {
  if (accounts.length === 0) return true;
  return !status.configured && accounts.some((a) => a.provider === "gmail");
}
