// The sidebar's mailbox list and what each mailbox is called (pure;
// tests/mailboxes.test.ts). Under All accounts (or a profile) the views
// have Penguin's names. Picking one account scopes the sidebar to it, and
// its mailboxes take the names its provider uses: Gmail's "All Mail",
// Outlook's "Junk Email" and "Deleted Items", an IMAP account's "Archive".
import type { IconName } from "../components/Icon";
import type { Account, Label, MailboxView } from "./types";
import { isFolderId, serviceName } from "./capabilities.ts";

/** One row of the sidebar's views. */
export interface MailboxItem {
  view: MailboxView;
  label: string;
  icon: IconName;
  /** G sequence ("g i"). */
  keys?: string;
}

/** Whose names an account's mailboxes take. Gmail quick setup (IMAP to Gmail) counts as Gmail. */
export type MailboxFlavor = "gmail" | "imap" | "microsoft";

type AccountLike = Pick<Account, "provider" | "providerConfig">;

export function mailboxFlavor(a: AccountLike): MailboxFlavor {
  if (a.provider === "microsoft") return "microsoft";
  return serviceName(a) === "Gmail" ? "gmail" : "imap";
}

const GENERIC: Record<string, string> = {
  inbox: "Inbox",
  replyLater: "Reply Later",
  followUp: "Follow up",
  starred: "Starred",
  snoozed: "Snoozed",
  sent: "Sent",
  drafts: "Drafts",
  done: "Done",
  all: "All mail",
  spam: "Spam",
  trash: "Trash",
};

/**
 * What a mailbox is called: the provider's name inside one account, Penguin's
 * otherwise (null = All accounts or a profile). Views without a fixed name
 * (labels, smart views) return null.
 */
export function mailboxName(kind: MailboxView["kind"], a: AccountLike | null): string | null {
  const generic = GENERIC[kind] ?? null;
  if (!a || !generic) return generic;
  switch (mailboxFlavor(a)) {
    case "gmail":
      return kind === "all" ? "All Mail" : generic;
    case "imap":
      if (kind === "done") return "Archive";
      // iCloud says Junk; Yahoo and AOL call the folder Bulk but their apps say Spam.
      if (kind === "spam") return a.providerConfig?.host === "icloud" ? "Junk" : "Spam";
      return generic;
    case "microsoft":
      if (kind === "done") return "Archive";
      if (kind === "sent") return "Sent Items";
      if (kind === "spam") return "Junk Email";
      if (kind === "trash") return "Deleted Items";
      return generic;
  }
}

const item = (kind: MailboxView["kind"], icon: IconName, keys: string | undefined, a: AccountLike | null): MailboxItem => ({
  view: { kind } as MailboxView,
  label: mailboxName(kind, a)!,
  icon,
  keys,
});

/**
 * The sidebar's views. `account` = the one account the sidebar is scoped to
 * (null: All accounts or a profile); `hasSpam` = some account in view has a
 * Spam (Junk) folder, which an IMAP server may not.
 */
export function sidebarMailboxes(account: AccountLike | null, hasSpam: boolean): MailboxItem[] {
  const flavor = account ? mailboxFlavor(account) : null;
  const out: (MailboxItem | false)[] = [
    item("inbox", "inbox", "g i", account),
    item("replyLater", "replyLater", "g y", account),
    item("followUp", "followUp", "g f", account),
    item("starred", "star", "g s", account),
    item("snoozed", "snooze", "g h", account),
    item("sent", "send", "g t", account),
    item("drafts", "draft", "g d", account),
    // Gmail keeps Penguin's Done and adds its All Mail; elsewhere the Archive
    // folder is what Done lists (out of the inbox).
    item("done", flavor === "imap" || flavor === "microsoft" ? "archive" : "done", "g e", account),
    flavor === "gmail" && item("all", "mails", "g a", account),
    hasSpam && item("spam", "shield", "g !", account),
    item("trash", "trash", "g #", account),
  ];
  return out.filter((x): x is MailboxItem => !!x);
}

/** Whether the accounts in view have a Spam folder (labels not loaded yet: assume so). */
export function hasSpamFolder(labels: readonly Pick<Label, "accountId" | "id">[], accountIds: readonly string[] | null, loaded: boolean): boolean {
  if (!loaded) return true;
  return labels.some((l) => l.id === "SPAM" && (!accountIds || accountIds.includes(l.accountId)));
}

/** The sidebar's labels section title for the labels shown, scoped to `account` or not. */
export function labelsTitle(account: AccountLike | null, labels: readonly Pick<Label, "id">[]): string {
  if (account) {
    switch (mailboxFlavor(account)) {
      case "gmail":
        // Gmail quick setup's labels arrive as f: folders; they're still labels.
        return "Labels";
      case "imap":
        return "Folders";
      case "microsoft": {
        const folders = labels.some((l) => isFolderId(l.id));
        const categories = labels.some((l) => !isFolderId(l.id));
        return folders && categories ? "Folders & categories" : categories ? "Categories" : "Folders";
      }
    }
  }
  return labels.length > 0 && labels.every((l) => isFolderId(l.id)) ? "Folders" : "Labels";
}

/** The service a scoped sidebar names under the account: "Gmail", "Fastmail", "Outlook", or "IMAP" for a server by host name. */
export function serviceTag(a: AccountLike): string {
  const s = serviceName(a);
  return a.provider === "imap" && s.includes(".") ? "IMAP" : s;
}
