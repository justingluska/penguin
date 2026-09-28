// Text snippets for the composer: typed as ";trigger", or picked with ⌘;.
//
// Variables in a snippet body:
//   {first_name}     first name of the first To recipient
//   {last_name}      their last name
//   {full_name}      their whole name
//   {company}        their company, from the address's domain (not for
//                    Gmail, Outlook, iCloud and other personal mail)
//   {sender_name}    full name of the person whose message you're replying to
//   {my_name}        your name on the sending account
//   {my_first_name}  its first word
//   {date}           today's date ("September 24, 2026")
//   {cursor}         where the caret lands after inserting
// Any other {placeholder} stays in the text; Tab jumps between them, and
// sending asks first while one is left (`unfilledPlaceholders`).
//
// A snippet can also carry a subject (used when the message has none), Cc and
// Bcc recipients and files; `snippetExtras` works out what to add.
import type { Address, Snippet } from "../../lib/types";
import { displayName } from "../../lib/format.ts";

export type { Snippet } from "../../lib/types";

export const SNIPPET_VARIABLES: Array<[string, string]> = [
  ["{first_name}", "First name of the first recipient"],
  ["{last_name}", "Last name of the first recipient"],
  ["{full_name}", "Full name of the first recipient"],
  ["{company}", "The recipient's company, from their email domain"],
  ["{sender_name}", "Name of the person you're replying to"],
  ["{my_name}", "Your name on the sending account"],
  ["{my_first_name}", "Your first name"],
  ["{date}", "Today's date"],
  ["{cursor}", "Where the cursor lands"],
];

export interface SnippetContext {
  to: Address[];
  replyingTo: Address | null;
  myName: string;
  now?: Date;
}

/** A person's name split in two; null parts when unknown (an address with no name). */
export function nameParts(a: Address | undefined | null): { first: string | null; last: string | null; full: string | null } {
  const raw = a?.name?.trim().replace(/^"+|"+$/g, "").trim() ?? "";
  if (!raw || raw.includes("@")) return { first: null, last: null, full: null };
  // "Raman, Priya" is written last-name first.
  const comma = /^([^,]+),\s*([^,]+)$/.exec(raw);
  const words = (comma ? `${comma[2]} ${comma[1]}` : raw).split(/\s+/).filter(Boolean);
  if (words.length === 0) return { first: null, last: null, full: null };
  return { first: words[0], last: words.length > 1 ? words[words.length - 1] : null, full: words.join(" ") };
}

/** Domains of personal mail services: they say nothing about where someone works. */
const PERSONAL_DOMAINS = new Set([
  "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com", "yahoo.com", "ymail.com",
  "aol.com", "icloud.com", "me.com", "mac.com", "proton.me", "protonmail.com", "pm.me", "fastmail.com",
  "gmx.com", "gmx.net", "gmx.de", "web.de", "mail.com", "zoho.com", "yandex.com", "yandex.ru", "hey.com",
  "tutanota.com", "tuta.io", "qq.com", "163.com", "126.com", "naver.com", "hotmail.co.uk", "yahoo.co.uk",
  "outlook.fr", "hotmail.fr", "yahoo.fr", "orange.fr", "free.fr", "libero.it", "t-online.de",
]);

/** Second-level labels that belong to the suffix ("co.uk", "com.au"). */
const SUFFIX_LABELS = new Set(["co", "com", "org", "net", "ac", "gov", "edu", "ne", "or"]);

/** "priya@mail.northwind.example" → "Northwind"; null for personal mail or no domain. */
export function companyFromEmail(email: string | undefined | null): string | null {
  const domain = email?.split("@")[1]?.trim().toLowerCase();
  if (!domain || PERSONAL_DOMAINS.has(domain)) return null;
  const labels = domain.split(".").filter(Boolean);
  if (labels.length < 2) return null;
  let i = labels.length - 2;
  if (i > 0 && SUFFIX_LABELS.has(labels[i]) && labels[labels.length - 1].length === 2) i -= 1;
  const name = labels[i];
  if (!name || PERSONAL_DOMAINS.has(labels.slice(i).join("."))) return null;
  return name
    .split("-")
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

/** The value of each built-in variable (null: unknown here, the placeholder stays). */
export function snippetValues(ctx: SnippetContext): Record<string, string | null> {
  const to = ctx.to[0];
  const n = nameParts(to);
  const me = ctx.myName.trim();
  return {
    first_name: n.first,
    last_name: n.last,
    full_name: n.full,
    company: companyFromEmail(to?.email),
    sender_name: ctx.replyingTo ? displayName(ctx.replyingTo) : null,
    my_name: me || null,
    my_first_name: me ? me.split(/\s+/)[0] : null,
    date: (ctx.now ?? new Date()).toLocaleDateString("en-US", { month: "long", day: "numeric", year: "numeric" }),
  };
}

const VARIABLE = /\{(first_name|last_name|full_name|company|sender_name|my_name|my_first_name|date)\}/g;

/**
 * Fill known variables. Returns the text plus where the caret should go:
 * at {cursor} if present, else the first remaining {placeholder} (selected),
 * else the end. Unknown values (no recipient yet) stay as placeholders.
 */
export function expandSnippet(
  body: string,
  ctx: SnippetContext,
  opts: { keepCursor?: boolean } = {},
): { text: string; selStart: number; selEnd: number } {
  const vars = snippetValues(ctx);
  let text = body.replace(VARIABLE, (m, k: string) => vars[k] ?? m);
  const cursor = opts.keepCursor ? -1 : text.indexOf("{cursor}");
  if (cursor !== -1) {
    text = text.slice(0, cursor) + text.slice(cursor + "{cursor}".length);
    return { text, selStart: cursor, selEnd: cursor };
  }
  const v = /\{[^}\n]+\}/.exec(text);
  if (v) return { text, selStart: v.index, selEnd: v.index + v[0].length };
  return { text, selStart: text.length, selEnd: text.length };
}

/** Snippets for what's typed after ";" (or in the ⌘; picker): trigger prefix first, then title matches, most used first. */
export function matchSnippets(list: Snippet[], typed: string): Snippet[] {
  const t = typed.trim().toLowerCase();
  if (!t) return [...list].sort((a, b) => b.uses - a.uses);
  return list
    .filter((s) => s.trigger.startsWith(t) || (t.length > 1 && (s.title.toLowerCase().includes(t) || s.body.toLowerCase().includes(t))))
    .sort(
      (a, b) =>
        Number(b.trigger.startsWith(t)) - Number(a.trigger.startsWith(t)) ||
        Number(b.title.toLowerCase().includes(t)) - Number(a.title.toLowerCase().includes(t)) ||
        b.uses - a.uses,
    );
}

export function validTrigger(t: string): boolean {
  return /^[a-z0-9_-]{1,32}$/.test(t);
}

/**
 * {placeholders} still in the text when it's about to go (a snippet's {day}
 * nobody filled in). Only the curly-brace form snippets use: a word, spaces
 * allowed, no braces or line breaks inside, ≤ 40 characters.
 */
export function unfilledPlaceholders(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/\{([A-Za-z][A-Za-z0-9_ .'-]{0,39})\}/g)) {
    if (!out.includes(m[0])) out.push(m[0]);
  }
  return out;
}

/** What a snippet adds besides its text. */
export interface SnippetExtras {
  /** Set only when the message has no subject yet. */
  subject: string | null;
  cc: Address[];
  bcc: Address[];
}

/**
 * The subject, Cc and Bcc a snippet brings to the message: addresses already
 * on it (in any field) aren't added twice, unreadable ones are skipped.
 * `parse` reads "Name <a@b>" (draft.ts parseAddress).
 */
export function snippetExtras(
  s: Pick<Snippet, "subject" | "cc" | "bcc">,
  current: { subject: string; to: Address[]; cc: Address[]; bcc: Address[] },
  parse: (raw: string) => Address | null,
): SnippetExtras {
  const have = new Set([...current.to, ...current.cc, ...current.bcc].map((a) => a.email.toLowerCase()));
  const take = (list: string[]) => {
    const out: Address[] = [];
    for (const raw of list) {
      const a = parse(raw);
      if (!a || have.has(a.email.toLowerCase())) continue;
      have.add(a.email.toLowerCase());
      out.push(a);
    }
    return out;
  };
  const cc = take(s.cc ?? []);
  const bcc = take(s.bcc ?? []);
  const subject = s.subject?.trim() && !current.subject.trim() ? s.subject.trim() : null;
  return { subject, cc, bcc };
}
