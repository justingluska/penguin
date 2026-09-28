// Ready-made splits (Settings → Inbox → Add a split). Each is just a query in
// the search language, so everything a preset does can be edited after. The
// first three are what a new Split Inbox starts with (settings.rs
// default_inbox_splits): keep them the same there.
import type { InboxSplit } from "../../lib/types";
import { isConsumerAddress } from "../../lib/format.ts";
import type { IconName } from "../../components/Icon";

export interface SplitPreset {
  key: string;
  name: string;
  icon: IconName;
  /** One line in the Add menu. */
  blurb: string;
  /** The query, given your accounts' addresses (Team needs them); null = not available. */
  query: (myEmails: string[]) => string | null;
}

export const SPLIT_PRESETS: SplitPreset[] = [
  {
    key: "important",
    name: "Important",
    icon: "star",
    blurb: "What Gmail marks important, without newsletters or invitations. Gmail accounts only.",
    query: () => "is:important -is:newsletter -has:invite",
  },
  {
    key: "calendar",
    name: "Calendar",
    icon: "calendar",
    blurb: "Invitations and updates from Google Calendar and Calendly.",
    query: () => "has:invite OR from:calendar-notification@google.com OR from:@calendly.com",
  },
  {
    key: "news",
    name: "News",
    icon: "news",
    blurb: "Newsletters and other bulk mail (a List-Unsubscribe header or a promotions/updates category).",
    query: () => "is:newsletter",
  },
  {
    key: "people",
    name: "People",
    icon: "users",
    blurb: "Mail from people you've written to, without bulk mail. Works for every account.",
    query: () => "is:known-sender -is:newsletter",
  },
  {
    key: "vip",
    name: "VIP",
    icon: "user",
    blurb: "The people you pick. Add more from any message with ⌘K → Add sender to a split.",
    // Filled in by the editor from the people picked.
    query: () => null,
  },
  {
    key: "team",
    name: "Team",
    icon: "users",
    blurb: "People at your work domains (your non-personal accounts).",
    query: (mine) => {
      const domains = [...new Set(mine.filter((e) => !isConsumerAddress(e)).map((e) => e.split("@")[1]?.toLowerCase()).filter(Boolean))];
      return domains.length ? domains.map((d) => `from:@${d}`).join(" OR ") : null;
    },
  },
  {
    key: "tools",
    name: "Notifications",
    icon: "bell",
    blurb: "Automated mail from apps and services: no-reply and notification senders.",
    query: () => "from:noreply OR from:no-reply OR from:notifications OR from:notification OR from:alerts",
  },
  {
    key: "files",
    name: "Attachments",
    icon: "clip",
    blurb: "Conversations with a file attached.",
    query: () => "has:attachment -is:newsletter",
  },
];

/** A VIP split's query for these people. */
export function vipQuery(emails: string[]): string {
  return emails.map((e) => `from:${e.trim().toLowerCase()}`).join(" OR ");
}

/** The addresses a VIP-style query names (for editing it as people). Null when it's anything else. */
export function peopleOf(query: string): string[] | null {
  const parts = query.trim().split(/\s+OR\s+/);
  const out: string[] = [];
  for (const p of parts) {
    const m = /^from:([^\s@]+@[^\s@]+\.[^\s@]+)$/i.exec(p.trim());
    if (!m) return null;
    out.push(m[1].toLowerCase());
  }
  return out.length ? out : null;
}

/** A split from a preset (VIP: from `people`). Null when it can't be made (no work domain, no people). */
export function splitFromPreset(p: SplitPreset, id: string, myEmails: string[], people: string[] = []): InboxSplit | null {
  const query = p.key === "vip" ? (people.length ? vipQuery(people) : null) : p.query(myEmails);
  return query ? { id, name: p.name, query, hideWhenEmpty: false } : null;
}
