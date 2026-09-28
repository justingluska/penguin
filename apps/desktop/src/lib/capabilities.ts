// What an account's provider is called and what it can do, for the UI to
// name it right and offer only what works (Account.capabilities, see
// docs/PROVIDERS-IMPL.md → Capabilities). Pure: tests/capabilities.test.ts.
import type { Account, Capabilities, Label, MailboxView } from "./types";

const HOST_NAMES: Record<string, string> = {
  gmail: "Gmail",
  googleWorkspace: "Gmail",
  outlookPersonal: "Outlook",
  microsoft365: "Microsoft 365",
  yahoo: "Yahoo",
  aol: "AOL",
  icloud: "iCloud",
  fastmail: "Fastmail",
};

const isLocal = (host: string | undefined) => !!host && /^(127\.0\.0\.1|localhost|::1)$/i.test(host);

/**
 * The service an account's mail lives on, as people call it: "Gmail",
 * "iCloud", "Proton Mail Bridge", or the IMAP server's host name.
 */
export function serviceName(a: Pick<Account, "provider" | "providerConfig"> | null | undefined): string {
  if (!a) return "Gmail";
  const host = a.providerConfig?.host ?? "";
  if (a.provider === "gmail") return "Gmail";
  if (a.provider === "microsoft") return HOST_NAMES[host] ?? "Microsoft";
  if (HOST_NAMES[host]) return HOST_NAMES[host];
  const server = a.providerConfig?.imap?.host;
  if (isLocal(server)) return "Proton Mail Bridge";
  return server || "your mail server";
}

// ---------------------------------------------------------------------------
// Gating: offer an action only when every account it touches can do it
// ---------------------------------------------------------------------------

type Caps = Pick<Account, "capabilities"> | null | undefined;

/** True when there is at least one account and each can `cap` (an unknown account can't). */
export function everyAccount(accounts: readonly Caps[], cap: keyof Capabilities): boolean {
  return accounts.length > 0 && accounts.every((a) => !!a?.capabilities?.[cap]);
}

export function anyAccount(accounts: readonly Caps[], cap: keyof Capabilities): boolean {
  return accounts.some((a) => !!a?.capabilities?.[cap]);
}

export interface ThreadAbilities {
  /** "Label as…": add and remove labels (Gmail labels, Microsoft categories). */
  label: boolean;
  /** "Move to…": put the conversation in one folder (IMAP folders, Microsoft folders). */
  move: boolean;
}

/** What the conversations of these accounts offer; a mixed selection gets only what all share. */
export function threadAbilities(accounts: readonly Caps[]): ThreadAbilities {
  return { label: everyAccount(accounts, "labels"), move: everyAccount(accounts, "folders") };
}

/** Folders arrive as user labels with ids "f:<percent-encoded path>" (docs/PROVIDERS-IMPL.md §4). */
export const isFolderId = (id: string): boolean => id.startsWith("f:");

/** A label "Label as…" lists: a user label of a labels account, minus its folders. */
export function isLabelChoice(l: Pick<Label, "kind" | "id">, account: Caps): boolean {
  const c = account?.capabilities;
  return l.kind === "user" && !!c?.labels && !(c.folders && isFolderId(l.id));
}

/** A label "Move to…" lists (besides the Inbox): a folder of a folders account. */
export function isFolderChoice(l: Pick<Label, "kind" | "id">, account: Caps): boolean {
  return l.kind === "user" && !!account?.capabilities?.folders && isFolderId(l.id);
}

/** Label editing in the sidebar and Settings, per the label's account. */
export function labelEditing(account: Caps): { edit: boolean; colors: boolean } {
  return { edit: !!account?.capabilities?.labelEdit, colors: !!account?.capabilities?.labelColors };
}

/** Split-inbox tabs need IMPORTANT/CATEGORY_* labels, which only some providers have. */
export function inboxTabsAvailable(accountsInView: readonly Caps[]): boolean {
  return anyAccount(accountsInView, "inboxCategories");
}

// ---------------------------------------------------------------------------
// Move to…: label deltas
// ---------------------------------------------------------------------------

export const INBOX = "INBOX";

export interface LabelDelta {
  add: string[];
  remove: string[];
}

/**
 * Moving a conversation to `target` (a folder id, or INBOX): add the target,
 * and take it out of the inbox and every other folder it's in. Moving to
 * the inbox takes it out of its folders only.
 */
export function moveDelta(labelIds: readonly string[], target: string): LabelDelta {
  const others = labelIds.filter((id) => isFolderId(id) && id !== target);
  return target === INBOX ? { add: [INBOX], remove: others } : { add: [target], remove: [INBOX, ...others] };
}

/** The labels after a delta. */
export function applyDelta(labelIds: readonly string[], d: LabelDelta): string[] {
  const out = labelIds.filter((id) => !d.remove.includes(id));
  for (const id of d.add) if (!out.includes(id)) out.push(id);
  return out;
}

/** The delta that puts back what `d` changed on a conversation that had `before`. */
export function undoDelta(before: readonly string[], d: LabelDelta): LabelDelta {
  return { add: d.remove.filter((id) => before.includes(id)), remove: d.add.filter((id) => !before.includes(id)) };
}

/**
 * One backend call per label: which conversations gain it and which lose it.
 * The adds go first, so a conversation is never left in no folder at all.
 */
export function deltaCalls<R>(items: readonly { ref: R; delta: LabelDelta }[]): { add: Map<string, R[]>; remove: Map<string, R[]> } {
  const add = new Map<string, R[]>();
  const remove = new Map<string, R[]>();
  const push = (m: Map<string, R[]>, id: string, r: R) => m.set(id, [...(m.get(id) ?? []), r]);
  for (const { ref, delta } of items) {
    for (const id of delta.add) push(add, id, ref);
    for (const id of delta.remove) push(remove, id, ref);
  }
  return { add, remove };
}

/** Whether a conversation with these labels still belongs in the view (else the list drops the row). */
export function shownIn(view: MailboxView, labelIds: readonly string[]): boolean {
  const has = (id: string) => labelIds.includes(id);
  switch (view.kind) {
    case "inbox":
      return has(INBOX);
    case "label":
      return has(view.labelId);
    case "done":
      return !has(INBOX) && !has("TRASH") && !has("SPAM");
    default:
      return true;
  }
}

/** "Your Gmail isn't affected." / "Your mail on iCloud isn't affected." (Remove account). */
export function mailboxUnaffected(a: Pick<Account, "provider" | "providerConfig">): string {
  const name = serviceName(a);
  return name === "Gmail" ? "Your Gmail isn't affected." : `Your mail on ${name} isn't affected.`;
}
