// Smart views: the catalog (what each built-in view is, its icon, its empty
// state) and the pure edits Settings, the sidebar and search make to
// Settings.smartViews. No React and no store imports, so it runs in node
// tests (tests/smartViews.test.ts). Data comes from penguin-core
// store_smart.rs through listThreads({kind: "smart"} / {kind: "query"}).
import type { IconName } from "../../components/Icon";
import type { CustomView, MailboxView, SmartFileKind, SmartViewId, SmartViewSettings } from "../../lib/types";
import { SMART_FILE_KINDS, SMART_VIEWS } from "../../lib/types.ts";

export interface SmartViewDef {
  id: SmartViewId;
  label: string;
  icon: IconName;
  /** One line for Settings: what's in it and where it comes from. */
  blurb: string;
  /** What the sidebar count counts. */
  counts: string;
  /** Words for the Settings search and ⌘K. */
  keywords: string[];
  empty: { title: string; body: string };
  /** Archiving takes a row out (the view lists inbox mail), like the inbox. */
  inboxLike?: boolean;
}

export const SMART_VIEW_DEFS: readonly SmartViewDef[] = [
  {
    id: "receipts",
    label: "Receipts",
    icon: "receipt",
    blurb: "Orders and receipts with the merchant and amount, and this month's total.",
    counts: "receipts this month",
    keywords: ["orders", "purchases", "spending", "money", "shopping"],
    empty: { title: "No receipts yet", body: "Order confirmations and receipts show up here with the merchant and amount, read on this Mac." },
  },
  {
    id: "travel",
    label: "Travel",
    icon: "plane",
    blurb: "Flights, hotels, rental cars and trains: upcoming trips first, then past ones.",
    counts: "upcoming bookings",
    keywords: ["flights", "trips", "hotels", "itinerary", "boarding", "rental car", "train"],
    empty: { title: "No trips found", body: "Flight, hotel, rental car and train confirmations show up here, grouped by trip." },
  },
  {
    id: "packages",
    label: "Packages",
    icon: "package",
    blurb: "Shipments with the carrier, status and expected date, the ones on the way first.",
    counts: "parcels on the way",
    keywords: ["deliveries", "shipping", "tracking", "parcels", "ups", "fedex", "usps", "dhl"],
    empty: { title: "Nothing on the way", body: "Shipping and delivery emails show up here with their tracking number and status." },
  },
  {
    id: "bills",
    label: "Bills",
    icon: "bill",
    blurb: "Invoices and bills by due date, unpaid first and overdue ones highlighted.",
    counts: "unpaid bills",
    keywords: ["invoices", "due", "payments", "overdue", "utilities", "statements"],
    empty: { title: "No bills", body: "Invoices and bills show up here by due date. A later “payment received” email marks one paid." },
  },
  {
    id: "reservations",
    label: "Reservations",
    icon: "ticket",
    blurb: "Restaurant tables, event tickets and other bookings, upcoming first.",
    counts: "upcoming reservations",
    keywords: ["restaurants", "tickets", "events", "bookings", "concerts", "dinner"],
    empty: { title: "No reservations", body: "Restaurant bookings and event tickets show up here, upcoming first." },
  },
  {
    id: "invites",
    label: "Invites",
    icon: "invite",
    blurb: "Calendar invitations waiting for your answer, soonest first, with Yes / Maybe / No.",
    counts: "invitations to answer",
    keywords: ["invitations", "rsvp", "meetings", "calendar", "events"],
    empty: { title: "No invitations waiting", body: "Invitations you haven't answered yet show up here, soonest first." },
  },
  {
    id: "codes",
    label: "Codes",
    icon: "key",
    blurb: "Verification codes and sign-in links from the last two days, one click to copy.",
    counts: "codes from the last day",
    keywords: ["verification", "otp", "2fa", "one-time", "login", "sign-in", "magic link"],
    empty: { title: "No recent codes", body: "Verification codes and sign-in links from the last 48 hours show up here." },
  },
  {
    id: "files",
    label: "Files",
    icon: "clip",
    blurb: "Conversations with attachments, filtered by PDFs, images, documents or sheets.",
    counts: "unread conversations",
    keywords: ["attachments", "pdf", "images", "photos", "documents", "spreadsheets"],
    empty: { title: "No files", body: "Conversations with attachments show up here, newest first." },
  },
  {
    id: "newsletters",
    label: "Newsletters",
    icon: "news",
    blurb: "Newsletters in your inbox as a reading list. Done (E) takes one out.",
    counts: "unread newsletters",
    keywords: ["reading list", "bulk", "mailing lists", "unsubscribe", "digests"],
    empty: { title: "No newsletters", body: "Mailing-list mail in your inbox shows up here as a reading list." },
    inboxLike: true,
  },
  {
    id: "subscriptions",
    label: "Subscriptions",
    icon: "repeat",
    blurb: "Recurring charges found in your receipts: the same merchant at a steady cadence.",
    counts: "active subscriptions",
    keywords: ["recurring", "monthly", "yearly", "plans", "memberships", "charges"],
    empty: { title: "No subscriptions found", body: "A merchant that charges you at a steady cadence (three monthly receipts, or two a year apart) shows up here." },
  },
  {
    id: "people",
    label: "People you know",
    icon: "screener",
    blurb: "Inbox mail from people you've written to, newest first.",
    counts: "unread conversations",
    keywords: ["vip", "contacts", "important", "known senders", "friends"],
    empty: { title: "Nothing from people you know", body: "Inbox mail from anyone you've written to shows up here." },
    inboxLike: true,
  },
];

export const FILE_FILTERS: { kind: SmartFileKind | null; label: string }[] = [
  { kind: null, label: "All" },
  { kind: "pdf", label: "PDFs" },
  { kind: "images", label: "Images" },
  { kind: "docs", label: "Docs" },
  { kind: "sheets", label: "Sheets" },
];

export function smartDef(id: string): SmartViewDef | undefined {
  const base = id.split(":")[0];
  return SMART_VIEW_DEFS.find((d) => d.id === base);
}

export function isSmartViewId(id: string): id is SmartViewId {
  return (SMART_VIEWS as readonly string[]).includes(id);
}

/** A saved search's key in Settings.smartViews.shown / counts. */
export const customKey = (id: string) => `custom:${id}`;

/** One row of the sidebar's views group. */
export interface SmartSidebarItem {
  /** Its key in Settings.smartViews.shown ("receipts", "custom:<id>"). */
  key: string;
  view: MailboxView;
  label: string;
  icon: IconName;
  /** Its count is on (Settings → Views). */
  count: boolean;
  /** What smart_counts is asked for ("receipts", "query:<q>"). */
  countId: string;
  custom: CustomView | null;
}

/** The sidebar's views, in the user's order. */
export function sidebarSmartItems(s: SmartViewSettings): SmartSidebarItem[] {
  const out: SmartSidebarItem[] = [];
  for (const key of s.shown) {
    if (key.startsWith("custom:")) {
      const c = s.custom.find((x) => customKey(x.id) === key);
      if (!c) continue;
      out.push({
        key,
        view: { kind: "query", labelId: c.query },
        label: c.name || c.query,
        icon: "search",
        count: s.counts.includes(key),
        countId: `query:${c.query}`,
        custom: c,
      });
      continue;
    }
    const d = SMART_VIEW_DEFS.find((x) => x.id === key);
    if (!d) continue;
    out.push({ key, view: { kind: "smart", labelId: d.id }, label: d.label, icon: d.icon, count: s.counts.includes(key), countId: d.id, custom: null });
  }
  return out;
}

/** The sidebar row a view belongs to (Files with a filter is still Files). */
export function sidebarKeyOf(view: MailboxView, s: SmartViewSettings): string | null {
  if (view.kind === "smart") return view.labelId.split(":")[0];
  if (view.kind === "query") {
    const c = s.custom.find((x) => x.query === view.labelId);
    return c ? customKey(c.id) : null;
  }
  return null;
}

/** A smart view that lists inbox mail (archiving takes the row out). */
export function inboxLikeView(view: MailboxView): boolean {
  return view.kind === "smart" && !!smartDef(view.labelId)?.inboxLike;
}

/** The Files filter a view has on (null = all files). */
export function fileKindOf(view: MailboxView): SmartFileKind | null {
  if (view.kind !== "smart") return null;
  const [base, kind] = view.labelId.split(":");
  return base === "files" && (SMART_FILE_KINDS as readonly string[]).includes(kind) ? (kind as SmartFileKind) : null;
}

export function filesView(kind: SmartFileKind | null): MailboxView {
  return { kind: "smart", labelId: kind ? `files:${kind}` : "files" };
}

/** A view's title for the list header. */
export function smartTitle(view: MailboxView, s: SmartViewSettings): string {
  if (view.kind === "query") return s.custom.find((x) => x.query === view.labelId)?.name || view.labelId;
  if (view.kind === "smart") return smartDef(view.labelId)?.label ?? "View";
  return "";
}

// ---------------------------------------------------------------------------
// Edits (each returns a new SmartViewSettings; the caller saves it)
// ---------------------------------------------------------------------------

/** Turn a built-in view on (it goes last, or back where `SMART_VIEWS` puts it when nothing's on) or off. */
export function setShown(s: SmartViewSettings, key: string, on: boolean): SmartViewSettings {
  const has = s.shown.includes(key);
  if (on === has) return s;
  return { ...s, shown: on ? [...s.shown, key] : s.shown.filter((k) => k !== key) };
}

export function setCount(s: SmartViewSettings, key: string, on: boolean): SmartViewSettings {
  const has = s.counts.includes(key);
  if (on === has) return s;
  return { ...s, counts: on ? [...s.counts, key] : s.counts.filter((k) => k !== key) };
}

/** Move a shown view to `toIndex` among the others (the drag's drop index). */
export function moveView(s: SmartViewSettings, key: string, toIndex: number): SmartViewSettings {
  const from = s.shown.indexOf(key);
  if (from < 0) return s;
  const rest = s.shown.filter((k) => k !== key);
  const at = Math.max(0, Math.min(toIndex, rest.length));
  const shown = [...rest.slice(0, at), key, ...rest.slice(at)];
  return shown.every((k, i) => k === s.shown[i]) ? s : { ...s, shown };
}

/** Move one step up (-1) or down (1). */
export function moveViewBy(s: SmartViewSettings, key: string, delta: -1 | 1): SmartViewSettings {
  const i = s.shown.indexOf(key);
  return i < 0 ? s : moveView(s, key, i + delta);
}

export const MAX_CUSTOM_VIEWS = 30;
export const MAX_VIEW_NAME = 40;

/** A fresh id for a pinned search: [a-z0-9]. */
export function newCustomId(taken: string[], rand: () => number = Math.random): string {
  for (;;) {
    const id = "s" + Math.floor(rand() * 36 ** 6).toString(36).padStart(6, "0");
    if (!taken.includes(id)) return id;
  }
}

/** A short name for a query ("from:priya has:pdf" → "from:priya has:pdf", clipped). */
export function nameForQuery(q: string): string {
  const t = q.trim().replace(/\s+/g, " ");
  return t.length <= MAX_VIEW_NAME ? t : t.slice(0, MAX_VIEW_NAME - 1) + "…";
}

/**
 * Pin a search to the sidebar (the end of the views). Pinning a query that's
 * already pinned returns it unchanged. `null` when the query is blank or
 * there are too many.
 */
export function pinSearch(s: SmartViewSettings, query: string, rand?: () => number): { settings: SmartViewSettings; id: string } | null {
  const q = query.trim().replace(/\s+/g, " ");
  if (!q) return null;
  const same = s.custom.find((c) => c.query === q);
  if (same) return { settings: s, id: same.id };
  if (s.custom.length >= MAX_CUSTOM_VIEWS) return null;
  const id = newCustomId(s.custom.map((c) => c.id), rand);
  return {
    id,
    settings: { ...s, custom: [...s.custom, { id, name: nameForQuery(q), query: q }], shown: [...s.shown, customKey(id)] },
  };
}

export function isPinned(s: SmartViewSettings, query: string): boolean {
  const q = query.trim().replace(/\s+/g, " ");
  return !!q && s.custom.some((c) => c.query === q);
}

export function renameCustom(s: SmartViewSettings, id: string, name: string): SmartViewSettings {
  const n = name.trim().replace(/\s+/g, " ").slice(0, MAX_VIEW_NAME);
  return { ...s, custom: s.custom.map((c) => (c.id === id ? { ...c, name: n || c.query } : c)) };
}

/** Unpin: the view leaves the sidebar and Settings. */
export function removeCustom(s: SmartViewSettings, id: string): SmartViewSettings {
  const key = customKey(id);
  return {
    shown: s.shown.filter((k) => k !== key),
    counts: s.counts.filter((k) => k !== key),
    custom: s.custom.filter((c) => c.id !== id),
  };
}
