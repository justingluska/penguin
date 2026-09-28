// Natural-language search: "new senders last week", "emails I sent on august
// 27", "unanswered from nick this month", "pdfs from acme.example" become the
// operator query penguin-core runs (is:new-sender date:"last week", …). It is
// a fixed phrase grammar, local and deterministic: no model, no network.
//
// Conservative by design. A query is rewritten only when it carries a strong
// cue (a state such as "unanswered" or "new senders", "I sent", a file type in
// the plural, "unread"…), or a person together with a date or a file type.
// A date alone ("february") or a word alone stays a plain search (free text is
// never a date; the "Use … as a date?" hint covers that), and so does a bare
// "sent" after a name ("the pdf mike sent"). Operators, quoted phrases and
// exclusions the user typed pass through untouched; a query with OR or ( )
// is never rewritten. The UI shows the result as "Understood as …" with a way
// back to the plain words, so a wrong guess costs one click.

import { parseDatePhrase } from "./dates.ts";
import { tokenize } from "./query.ts";

export interface NlOptions {
  now?: Date;
  /** A known correspondent's name word or address ("from nick" alone is then enough). */
  isPerson?: (word: string) => boolean;
}

type Strength = "strong" | "type" | "person" | "date";

interface Cue {
  words: string[];
  op: string;
  strength: Strength;
}

const cue = (phrases: string[], op: string, strength: Strength = "strong"): Cue[] =>
  phrases.map((p) => ({ words: p.split(" "), op, strength }));

const SENT = "in:sent";

/** Phrase → operator. Longest match wins at each position. */
const CUES: Cue[] = [
  ...cue(
    [
      "new senders",
      "new sender",
      "new people",
      "new contacts",
      "first time senders",
      "first-time senders",
      "first contact",
      "first contacts",
      "emailed me for the first time",
      "wrote to me for the first time",
      "wrote me for the first time",
      "people who emailed me first",
    ],
    "is:new-sender",
  ),
  ...cue(
    [
      "new recipients",
      "new recipient",
      "new outbound",
      "first outbound",
      "first emails i sent",
      "first email i sent",
      "first messages i sent",
      "people i emailed for the first time",
      "i emailed for the first time",
      "emailed for the first time",
      "first time i emailed",
    ],
    "is:first-outbound",
  ),
  ...cue(
    [
      "unanswered",
      "unreplied",
      "not replied",
      "not replied to",
      "i haven't replied to",
      "i havent replied to",
      "i haven't answered",
      "i havent answered",
      "i didn't reply to",
      "i didnt reply to",
      "i never replied to",
      "i never answered",
      "needs a reply",
      "need a reply",
      "needing a reply",
      "needs reply",
      "still to answer",
    ],
    "is:unanswered",
  ),
  ...cue(
    [
      "awaiting reply",
      "awaiting a reply",
      "awaiting response",
      "awaiting a response",
      "waiting for a reply",
      "waiting for reply",
      "waiting on a reply",
      "waiting for a response",
      "no reply",
      "no reply yet",
      "no response",
      "no answer",
      "got no reply",
      "never got a reply",
      "that nobody answered",
    ],
    "is:awaiting",
  ),
  ...cue(["i replied to", "i answered", "that i replied to", "replied to", "replied"], "is:replied"),
  ...cue(
    [
      "i sent",
      "we sent",
      "i've sent",
      "ive sent",
      "sent by me",
      "i wrote",
      "i emailed",
      "my sent",
      "sent mail",
      "sent emails",
      "sent email",
      "sent messages",
      "sent items",
      "sent folder",
      "outgoing",
      "outbound",
    ],
    SENT,
  ),
  ...cue(["to me", "sent to me", "addressed to me"], "to:me"),
  ...cue(["from me", "by me"], "from:me"),
  ...cue(["unread", "not read"], "is:unread"),
  ...cue(["starred", "flagged"], "is:starred"),
  ...cue(["important"], "is:important"),
  ...cue(["snoozed"], "is:snoozed"),
  ...cue(["newsletters", "newsletter", "mailing lists", "mailing list", "bulk mail", "promotional"], "is:newsletter"),
  ...cue(["drafts", "my drafts"], "in:drafts"),
  ...cue(["archived", "in the archive"], "in:done"),
  ...cue(["in trash", "in the trash", "trashed"], "in:trash"),
  ...cue(["in spam", "in junk"], "in:spam"),
  ...cue(["including trash", "including spam", "including trash and spam", "in trash and spam"], "in:anywhere"),
  ...cue(
    ["with attachments", "with an attachment", "with attachment", "with files", "attachments", "attached files", "with a file"],
    "has:attachment",
  ),
  ...cue(["pdfs", "pdf files", "with a pdf", "with pdfs", "pdf attachments"], "has:pdf"),
  ...cue(["images", "photos", "pictures", "screenshots", "with photos", "with images", "with a photo"], "has:image"),
  // Not "docs", "sheets", "slides": "google docs" is usually a search for the words.
  ...cue(["spreadsheets", "excel files"], "has:spreadsheet"),
  ...cue(["documents", "word docs", "word documents"], "has:doc"),
  ...cue(["decks", "slide decks", "presentations"], "has:presentation"),
  ...cue(["invites", "invitations", "calendar invites", "meeting invites", "meeting requests"], "has:invite"),
  ...cue(["with links", "with a link"], "has:link"),
  // Not bare "codes": "promo codes" are something else.
  ...cue(
    ["verification codes", "login codes", "sign-in codes", "2fa codes", "one-time codes", "security codes", "otps", "sign-in links", "magic links"],
    "has:otp",
  ),
  ...cue(["large attachments", "big attachments", "large files", "big files", "huge files", "large emails", "big emails", "heavy emails"], "larger:5M"),
  ...cue(["long threads", "long conversations", "long email threads"], "messages:>5"),
  ...cue(["on weekends", "on the weekend", "weekends", "at the weekend"], "day:weekend"),
  ...cue(["on weekdays", "weekdays", "during the week"], "day:weekday"),
  ...["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"].flatMap((d) => cue([`on ${d}s`, `${d}s`], `day:${d}`)),
  // Singular file words: weak, need a person or a date alongside.
  ...cue(["pdf", "a pdf"], "has:pdf", "type"),
  ...cue(["photo", "image", "picture", "screenshot"], "has:image", "type"),
  ...cue(["spreadsheet"], "has:spreadsheet", "type"),
  ...cue(["invite", "invitation"], "has:invite", "type"),
  ...cue(["attachment", "file"], "has:attachment", "type"),
].sort((a, b) => b.words.length - a.words.length);

/** Words that carry no meaning in a search phrase. */
const FILLER = new Set([
  "show",
  "find",
  "search",
  "get",
  "list",
  "me",
  "all",
  "my",
  "email",
  "emails",
  "mail",
  "mails",
  "message",
  "messages",
  "thread",
  "threads",
  "conversation",
  "conversations",
  "please",
  "the",
  "any",
  "some",
  "that",
  "which",
  "who",
  "i",
  "were",
  "was",
  "got",
  "received",
  "about",
  "for",
  "and",
  "or",
  "a",
  "an",
  "of",
  "stuff",
  "with",
  "things",
]);

/** "larger than 5mb", "over 10 MB", "smaller than 200k", "under 1mb". */
const SIZE_WORDS: Record<string, "larger" | "smaller"> = {
  larger: "larger",
  bigger: "larger",
  over: "larger",
  above: "larger",
  smaller: "smaller",
  under: "smaller",
  below: "smaller",
};

function sizeAt(w: string[], i: number): { n: number; op: string } | null {
  const kind = SIZE_WORDS[w[i]];
  if (!kind) return null;
  let j = i + 1;
  if (w[j] === "than") j++;
  const joined = `${w[j] ?? ""}${/^\d+(\.\d+)?$/.test(w[j] ?? "") && /^(k|kb|m|mb|g|gb)$/.test(w[j + 1] ?? "") ? w[j + 1] : ""}`;
  const m = /^(\d+(?:\.\d+)?)(k|kb|m|mb|g|gb)$/.exec(joined);
  if (!m) return null;
  const unit = m[2][0].toUpperCase();
  const used = joined === w[j] ? j + 1 - i : j + 2 - i;
  return { n: used, op: `${kind}:${m[1]}${unit}` };
}

const dateLike = (phrase: string, now: Date) => parseDatePhrase(phrase, now, true) !== null;

/** A date phrase starting at i: the longest run (≤ 6 words) that reads as one. */
function dateAt(w: string[], i: number, now: Date): { n: number; op: string } | null {
  for (let len = Math.min(6, w.length - i); len >= 1; len--) {
    let words = w.slice(i, i + len);
    let skip = 0;
    // "on august 27", "during march", "from last week" (a period, not "since").
    if (/^(on|during|from|in)$/.test(words[0]) && words.length > 1) {
      const rest = words.slice(1);
      const isRange = rest.some((x) => /^(to|through|until|-|–)$/.test(x));
      if (!(words[0] === "from" && isRange) && !(words[0] === "in" && dateLike(words.join(" "), now))) {
        words = rest;
        skip = 1;
      }
    }
    if (words.length === 1 && (words[0] === "may" || /\d/.test(words[0]))) continue;
    // A weekday alone is a date only with "on"/"last" ("on monday"); plurals are day:.
    const phrase = words.join(" ");
    if (!dateLike(phrase, now)) continue;
    const value = words.length === 1 ? phrase : `"${phrase}"`;
    return { n: words.length + skip, op: `date:${value}` };
  }
  return null;
}

/** A word that can name a person or company after "from"/"to"/"with". */
function personWord(raw: string, lower: string): boolean {
  if (FILLER.has(lower) || lower === "me" || lower === "to" || lower === "from" || lower === "with") return false;
  // "by May 15", "from monday": a date, not a person.
  if (parseDatePhrase(lower, new Date(), false) !== null) return false;
  return /^[\p{L}\p{N}][\p{L}\p{N}.@+_'&-]*$/u.test(raw);
}

interface Piece {
  text: string;
  op: boolean;
  /** The characters of the input it came from. */
  from: number;
  to: number;
}

export interface Interpretation {
  /** The operator query to run. */
  query: string;
  /** Each operator in it and the typed words it stands for (to remove a chip's words). */
  parts: Array<{ text: string; from: number; to: number }>;
}

/**
 * The operator query for a natural-language search, or null when the input
 * doesn't clearly read as one (then it is searched as typed).
 */
export function interpret(input: string, opts: NlOptions = {}): string | null {
  return interpretDetailed(input, opts)?.query ?? null;
}

/** `interpret`, plus where in the input each operator came from. */
export function interpretDetailed(input: string, opts: NlOptions = {}): Interpretation | null {
  const now = opts.now ?? new Date();
  const toks = tokenize(input).filter((t) => t.kind !== "space");
  if (!toks.length || toks.some((t) => t.kind === "or" || t.kind === "paren")) return null;
  // Pasted prose (a sentence from the mail, a whole subject line) is content
  // to find, not a command: more than a dozen words, or a sentence break.
  const plainWords = toks.filter((t) => t.kind === "word").length;
  if (plainWords > 12 || /[a-z0-9][.!?]\s+\S/i.test(input)) return null;

  const pieces: Piece[] = [];
  let strong = 0;
  let person: string | null = null;
  let hasDate = false;
  let hasType = false;
  let sentCue = false;

  // Plain words are interpreted in runs between operators/phrases/exclusions.
  let k = 0;
  while (k < toks.length) {
    const t = toks[k];
    if (t.kind !== "word" || t.negated) {
      pieces.push({ text: t.text, op: true, from: t.start, to: t.end });
      if (t.kind === "op" && /^(from|to|cc|bcc|with)$/.test(t.op)) person = person ?? t.value;
      if (t.kind === "op" && (t.op === "date" || t.op === "on" || t.op === "before" || t.op === "after")) hasDate = true;
      k++;
      continue;
    }
    let e = k;
    while (e < toks.length && toks[e].kind === "word" && !(toks[e] as { negated: boolean }).negated) e++;
    const raw = toks.slice(k, e).map((x) => x.text.replace(/[,;!?]+$/, "").replace(/\.$/, ""));
    const w = raw.map((x) => x.toLowerCase());
    const run = toks.slice(k, e);
    const at = (i0: number, n: number) => ({ from: run[i0].start, to: run[Math.min(run.length, i0 + n) - 1].end });
    let i = 0;
    while (i < w.length) {
      const prevFree = pieces.length > 0 && !pieces[pieces.length - 1].op;
      // Sizes: "larger than 5mb".
      const size = sizeAt(w, i);
      if (size) {
        pieces.push({ text: size.op, op: true, ...at(i, size.n) });
        strong++;
        i += size.n;
        continue;
      }
      // Phrases.
      const c = CUES.find((cu) => cu.words.every((x, j) => w[i + j] === x));
      if (c) {
        // A bare "sent" right after a name is theirs ("the pdf mike sent").
        pieces.push({ text: c.op, op: true, ...at(i, c.words.length) });
        if (c.strength === "strong") strong++;
        if (c.strength === "type") hasType = true;
        // Mail you sent: a following "to X" is its recipient.
        if (c.op === SENT || c.op === "is:awaiting" || c.op === "is:first-outbound") sentCue = true;
        i += c.words.length;
        // "I emailed nick", "i wrote to ana": the next name is the recipient.
        if (c.op === SENT && (w[i - 1] === "emailed" || w[i - 1] === "wrote")) {
          const toAt = i;
          if (w[i] === "to") i++;
          if (i < w.length && personWord(raw[i], w[i]) && !dateAt(w, i, now) && !CUES.some((cu) => cu.words[0] === w[i])) {
            pieces.push({ text: `to:${raw[i]}`, op: true, ...at(toAt, i + 1 - toAt) });
            person = person ?? raw[i];
            i++;
          }
        }
        continue;
      }
      if (w[i] === "sent" && !prevFree) {
        pieces.push({ text: SENT, op: true, ...at(i, 1) });
        strong++;
        sentCue = true;
        i++;
        continue;
      }
      // Dates: "last week", "on august 27", "from march" (a period).
      const d = dateAt(w, i, now);
      if (d) {
        pieces.push({ text: d.op, op: true, ...at(i, d.n) });
        hasDate = true;
        i += d.n;
        continue;
      }
      // People: "from nick", "sent by ana", "with dana", "to priya" (after a sent cue).
      const lead = w[i];
      const next = i + 1 < w.length ? i + 1 : -1;
      if (next !== -1 && (lead === "from" || lead === "by" || lead === "with" || lead === "to") && personWord(raw[next], w[next])) {
        const toOk = lead !== "to" || sentCue || (pieces.length > 0 && pieces[pieces.length - 1].text === SENT) || w.slice(i + 2).some((x) => x === "sent");
        if (toOk && !CUES.some((cu) => cu.words.every((x, j) => w[next + j] === x))) {
          const op = lead === "to" ? "to" : lead === "with" ? "with" : "from";
          pieces.push({ text: `${op}:${raw[next]}`, op: true, ...at(i, 2) });
          person = person ?? raw[next];
          if (lead === "to") sentCue = true;
          i += 2;
          continue;
        }
      }
      // "pdfs or images": either kind.
      if ((w[i] === "or" || w[i] === "and") && pieces.length && /^has:/.test(pieces[pieces.length - 1].text)) {
        const after = CUES.find((cu) => cu.words.every((x, j) => w[i + 1 + j] === x));
        if (after && /^has:/.test(after.op)) {
          pieces.push({ text: "OR", op: true, ...at(i, 1) });
          i++;
          continue;
        }
      }
      if (FILLER.has(w[i])) {
        i++;
        continue;
      }
      pieces.push({ text: raw[i], op: false, ...at(i, 1) });
      i++;
    }
    k = e;
  }

  const knownPerson = person !== null && (/[@.]/.test(person) || (opts.isPerson?.(person.toLowerCase()) ?? false));
  const confident = strong > 0 || (person !== null && (hasDate || hasType || knownPerson)) || (hasType && hasDate);
  if (!confident) return null;
  // Operators that came from words (not typed): at least one, or nothing changed.
  const seen = new Set<string>();
  const kept = pieces.filter((p) => {
    if (!p.op || p.text === "OR") return true;
    const key = p.text.toLowerCase();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  const out = kept
    .map((p) => p.text)
    .join(" ")
    .replace(/(^| )OR( OR)+/g, "$1OR")
    .replace(/^OR | OR$/g, "")
    .trim();
  if (!out || out === input.trim().replace(/\s+/g, " ")) return null;
  return { query: out, parts: kept.filter((p) => p.op && p.text !== "OR").map(({ text, from, to }) => ({ text, from, to })) };
}

/**
 * The input without the words behind one operator of its interpretation
 * (removing the "Last week" chip of "from mike last week pdf" leaves
 * "from mike pdf"). Null when `raw` isn't one of them.
 */
export function withoutPart(input: string, it: Interpretation, raw: string): string | null {
  const part = it.parts.find((p) => p.text.toLowerCase() === raw.trim().toLowerCase());
  if (!part) return null;
  return (input.slice(0, part.from) + " " + input.slice(part.to)).replace(/\s+/g, " ").trim();
}
