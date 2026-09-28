import type { AccountProvider } from "../../lib/types";

/**
 * Who "Also search …" asks, named for the accounts in scope: "Gmail",
 * "Outlook", "your mail server" (IMAP), or "the server" when they're mixed.
 */
export function serverName(providers: readonly AccountProvider[]): string {
  const kinds = new Set(providers);
  if (kinds.size !== 1) return "the server";
  const [only] = kinds;
  return only === "gmail" ? "Gmail" : only === "microsoft" ? "Outlook" : "your mail server";
}
