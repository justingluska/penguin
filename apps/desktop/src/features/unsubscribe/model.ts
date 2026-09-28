// What the Unsubscribe button says and does for an offer (pure; tested in
// tests/unsubscribe.test.ts). The offer comes from Rust
// (penguin_core::unsubscribe); see actions.ts for running it.
import type { UnsubscribeMethod, UnsubscribeOffer } from "../../lib/types";

export interface UnsubscribeCopy {
  /** Button text in the privacy row. */
  label: string;
  /** Button tooltip: where it goes. */
  tooltip: string;
  /** Dialog title: "Unsubscribe from Weekly?" */
  question: string;
  /** Dialog body: exactly what confirming does, one paragraph each. */
  explain: string[];
  /** The page that opens (method link), shown in full in the dialog. */
  url: string | null;
  /** The email that gets sent or composed (method mailto). */
  mail: { from: string; to: string; subject: string } | null;
  /** Confirm button. */
  verb: string;
  /**
   * What confirming runs: a backend method, or "compose" (a mailto for a
   * sender Gmail couldn't verify is only ever opened in the composer).
   */
  action: UnsubscribeMethod | "compose";
  /** Also offer "Edit first" (open the mailto in the composer). */
  editFirst: boolean;
  /** Already unsubscribed from this sender (one-click or email). */
  done: boolean;
}

/**
 * `account` is the receiving account's address (the From of a mailto
 * unsubscribe); omitted, the copy says "this account".
 */
export function unsubscribeCopy(offer: UnsubscribeOffer, sender: string, account?: string): UnsubscribeCopy {
  const done = !!offer.unsubscribed && offer.unsubscribed.method !== "link";
  const question = `Unsubscribe from ${sender}?`;
  const from = account || "this account";
  const base = { question, done, label: done ? "Unsubscribed" : "Unsubscribe", url: null, mail: null };
  switch (offer.method) {
    case "oneClick":
      return {
        ...base,
        tooltip: `One-click unsubscribe via ${offer.domain}`,
        explain: [
          `Penguin will send an unsubscribe request directly to ${offer.domain}. No page opens, and no email is sent from your account.`,
          "This is the one-click unsubscribe (RFC 8058) the sender set up for this list.",
        ],
        verb: "Unsubscribe",
        action: "oneClick",
        editFirst: false,
      };
    case "mailto": {
      const to = offer.mailto?.to ?? offer.domain;
      const subject = offer.mailto?.subject ?? "unsubscribe";
      const mail = { from, to, subject };
      if (!offer.verified) {
        return {
          ...base,
          mail,
          tooltip: `Unsubscribe by emailing ${to}`,
          explain: [
            `Gmail couldn't verify that this message came from ${sender}, so Penguin won't send the unsubscribe email for you.`,
            `Review email opens it in the composer, from ${from} to ${to} with the subject “${subject}”. Nothing is sent until you press Send.`,
          ],
          verb: "Review email",
          action: "compose",
          editFirst: false,
        };
      }
      return {
        ...base,
        mail,
        tooltip: `Unsubscribe by emailing ${to}`,
        explain: [
          `Penguin will send an email from ${from} to ${to} with the subject “${subject}”. The sender's list removes you when it arrives.`,
          "Nothing opens in your browser. Edit first opens the email in the composer instead.",
        ],
        verb: "Send",
        action: "mailto",
        editFirst: true,
      };
    }
    case "link":
      return {
        ...base,
        url: offer.linkUrl,
        tooltip: `Opens ${offer.domain} in your browser`,
        explain: [
          `This opens ${offer.domain} in your browser, where you finish unsubscribing.`,
          offer.source === "body"
            ? "Penguin found this link in the message's text (the sender didn't list one in the unsubscribe header), so check the address before you open it."
            : "The sender listed this page in the message's unsubscribe header.",
        ],
        verb: "Open page",
        action: "link",
        editFirst: false,
      };
  }
}

/** Gmail-style sender name for the confirm: display name, else the address. */
export function senderLabel(from: { name: string | null; email: string }): string {
  const name = from.name?.trim();
  return name || from.email;
}

/** A rule condition matching this sender's mail still in the inbox. */
export function senderCondition(email: string): string {
  const e = email.trim().toLowerCase();
  // Addresses with quotes or spaces can't be a bare from: value.
  return /^[^\s"()<>,;]+@[^\s"()<>,;]+$/.test(e) ? `from:${e} in:inbox` : `from:"${e.replace(/"/g, "")}" in:inbox`;
}
