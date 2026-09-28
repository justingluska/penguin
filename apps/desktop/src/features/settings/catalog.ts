// What Settings contains: its pages (the left nav) and a searchable index of
// the settings on each page, for the "Search settings" field. Pure data plus
// matching, so it runs in node tests. OWNER: settings agent.
//
// Adding a setting: a row whose `.setting-label` is literal text must have an
// entry here with that label (tests/settingsCatalog.test.ts scans the source
// and fails otherwise). The label is also how the row is found on its page to
// scroll to and highlight it (the start of the rendered `.setting-label` or
// `.st-sub` text), so keep it identical to what's shown. For a row without a
// label (a button, a list), give `find`: a CSS selector on that page.
//
// Adding keywords: put the words people might type but that aren't in the
// label in `keywords` (synonyms, other apps' names for it, what it affects).
// `desc` is the row's help text in brief; it's searched too, at a lower rank.
import type { IconName } from "../../components/Icon";
import type { SettingsSection } from "./state";

export interface SettingsPage {
  id: SettingsSection;
  label: string;
  icon: IconName;
  /** Words that find the page itself. */
  keywords?: string[];
}

export const SETTINGS_PAGES: SettingsPage[] = [
  { id: "you", label: "You", icon: "user", keywords: ["me", "photo", "picture", "avatar", "identity", "name"] },
  { id: "accounts", label: "Accounts", icon: "users", keywords: ["gmail", "google", "workspace", "email addresses", "inboxes"] },
  { id: "profiles", label: "Profiles", icon: "briefcase", keywords: ["group accounts", "work", "personal", "company", "switch"] },
  { id: "general", label: "General", icon: "sliders", keywords: ["appearance", "look", "behavior", "preferences"] },
  { id: "inbox", label: "Inbox", icon: "inbox", keywords: ["split inbox", "tabs", "splits", "inbox zero", "get to zero", "floe", "focus", "triage"] },
  { id: "sync", label: "Sync", icon: "refresh", keywords: ["download", "offline", "storage", "disk"] },
  { id: "calendar", label: "Calendar", icon: "calendar", keywords: ["events", "meetings", "invitations"] },
  { id: "compose", label: "Compose", icon: "compose", keywords: ["write", "writing", "editor", "reply", "send"] },
  { id: "signatures", label: "Signatures", icon: "pencil", keywords: ["sig", "sign off", "footer"] },
  { id: "privacy", label: "Privacy", icon: "shield", keywords: ["tracking", "security"] },
  { id: "search", label: "Search", icon: "search", keywords: ["index", "find"] },
  { id: "ai", label: "AI", icon: "sparkles", keywords: ["jev", "llm", "assistant", "machine learning"] },
  { id: "views", label: "Views", icon: "list", keywords: ["smart views", "sidebar", "receipts", "travel", "packages", "bills", "filtered lists", "saved searches"] },
  { id: "rules", label: "Rules", icon: "wand", keywords: ["filters", "automation", "automate", "auto label", "auto archive", "move", "forward"] },
  { id: "keyboard", label: "Keyboard", icon: "keyboard", keywords: ["shortcuts", "hotkeys", "keys"] },
  { id: "diagnostics", label: "Developer", icon: "database", keywords: ["diagnostics", "debug", "advanced", "logs"] },
  { id: "whatsnew", label: "What's new", icon: "news", keywords: ["changelog", "release notes", "version", "updates"] },
  { id: "about", label: "About", icon: "info", keywords: ["version", "credits", "author", "open source", "github", "twitter", "x"] },
];

export interface SettingEntry {
  page: SettingsSection;
  /** The row's label as shown (its `.setting-label` or `.st-sub` text). */
  label: string;
  /** A short description, searched at a lower rank. */
  desc?: string;
  keywords?: string[];
  /** CSS selector for the row on its page, when it has no label to find it by. */
  find?: string;
}

export const SETTINGS_INDEX: SettingEntry[] = [
  // You
  { page: "you", label: "Your photo", find: ".me-card", keywords: ["photo", "picture", "avatar", "crop", "upload", "change photo", "remove photo", "profile picture"] },
  { page: "you", label: "Use your Google profile photo", keywords: ["avatar", "picture", "copy photo"] },

  // Accounts
  { page: "accounts", label: "Account order", find: ".st-accounts", keywords: ["reorder", "drag", "sort", "move up", "move down", "arrange"] },
  { page: "accounts", label: "Account colors", find: ".account-color", keywords: ["color", "colour", "palette", "40 colors", "dot"] },
  { page: "accounts", label: "Account nicknames", find: ".acc-nick", keywords: ["nickname", "rename", "name", "display name"] },
  { page: "accounts", label: "Remove an account", find: ".account-remove", keywords: ["delete account", "sign out", "log out", "disconnect"] },
  { page: "accounts", label: "Add account", find: "[data-setting='add-account']", keywords: ["new account", "sign in", "connect", "login", "yahoo", "icloud", "outlook", "imap"] },
  { page: "accounts", label: "Unfinished setups", find: ".setting-account.is-pending", keywords: ["pending", "resume", "setting up"] },
  { page: "accounts", label: "Show in All Inboxes", desc: "Leave an account's mail out of All accounts", keywords: ["unified inbox", "all accounts", "hide account", "combined"] },
  { page: "accounts", label: "Sign-in method", desc: "The Google sign-in client Penguin uses", keywords: ["oauth", "client id", "google cloud", "ios client", "credentials"] },

  // Profiles
  { page: "profiles", label: "New profile", find: "[data-setting='profile-actions']", keywords: ["create profile", "add profile"] },
  { page: "profiles", label: "Suggest profiles by domain", find: "[data-setting='profile-actions']", keywords: ["automatic", "group by company"] },

  // General
  { page: "general", label: "Theme", desc: "Dark, light or system", keywords: ["dark", "dark mode", "light mode", "night", "appearance", "system"] },
  { page: "general", label: "Dark mode shade", desc: "Black, Charcoal, Dim or Navy", keywords: ["black", "charcoal", "dim", "navy", "gray", "grey", "midnight", "background", "night", "oled", "appearance"] },
  { page: "general", label: "Sidebar text size", desc: "Smaller or larger sidebar text", keywords: ["font", "font size", "text size", "bigger", "smaller", "zoom", "sidebar"] },
  { page: "general", label: "Sidebar theme", desc: "Tints the sidebar", keywords: ["color", "tint", "graphite", "midnight", "arctic", "lavender", "mint", "peach", "sky", "rose", "butter", "sage", "slate", "mauve", "sand", "mocha", "crimson", "amber", "olive", "forest", "teal", "ocean", "indigo", "plum"] },
  { page: "general", label: "Accent color", desc: "Caret, focus ring, unread dot and selection", keywords: ["accent", "highlight", "colour", "tint", "caret", "focus", "unread", "blue", "purple", "pink", "red", "orange", "yellow", "green", "teal", "graphite"] },
  { page: "general", label: "Corners", desc: "Rounded, subtle or square", keywords: ["corner", "radius", "rounded", "square", "sharp", "round"] },
  { page: "general", label: "Match accent to theme", keywords: ["accent", "caret", "focus ring", "unread marker", "blue"] },
  { page: "general", label: "Dark email bodies", desc: "HTML mail in the dark theme", keywords: ["dark mode", "invert", "html", "experimental"] },
  { page: "general", label: "Accounts in the sidebar", keywords: ["hide", "sidebar section"] },
  { page: "general", label: "Labels in the sidebar", keywords: ["hide", "folders", "sidebar section"] },
  { page: "general", label: "List style", desc: "Quiet, Classic, High contrast, Cards or Mail", keywords: ["conversation list", "message list", "rows", "look", "appearance", "quiet", "classic", "high contrast", "cards", "mail", "preview", "layout"] },
  { page: "general", label: "Density", desc: "Compact or comfortable rows", keywords: ["compact", "comfortable", "row height", "spacing"] },
  { page: "general", label: "Sender photos appear", desc: "In the message list, inside emails, both or off", keywords: ["avatars", "pictures", "faces"] },
  { page: "general", label: "Follow up after", desc: "When unanswered sent mail shows in Follow up", keywords: ["follow up", "nudge", "no reply", "reminder", "days", "reply later"] },
  { page: "general", label: "Show keyboard shortcut hints", keywords: ["key caps", "kbd", "hints", "shortcuts"] },
  { page: "general", label: "Notify me about new mail", keywords: ["notifications", "alerts", "banners", "badge", "new mail"] },
  { page: "general", label: "Only people I've emailed before", keywords: ["notifications", "vip", "strangers", "quiet"] },
  { page: "general", label: "Welcome setup", find: "[data-setting='welcome']", desc: "Run the first-run questions again", keywords: ["onboarding", "set up", "setup", "first run", "welcome", "search by meaning", "defaults"] },
  { page: "general", label: "Swipe actions", desc: "Two-finger swipe on a conversation", keywords: ["swipe", "gesture", "trackpad", "archive", "trash"] },

  // Inbox
  { page: "inbox", label: "Split Inbox", desc: "Tabs above the inbox, each a search", keywords: ["tabs", "splits", "important", "other", "vip", "team", "news", "newsletters", "calendar", "categories", "priority", "inbox tabs"] },
  { page: "inbox", label: "Splits", find: "[data-setting='splits']", desc: "Add, edit, reorder or remove splits", keywords: ["vip", "team", "custom split", "hide when empty", "presets", "notifications"] },
  { page: "inbox", label: "Getting to zero", keywords: ["inbox zero", "empty inbox"] },
  { page: "inbox", label: "Get to zero", desc: "Archive everything older than…", keywords: ["inbox zero", "bulk archive", "archive old", "clean up", "mark all done", "declutter"] },
  { page: "inbox", label: "Celebrate inbox zero", keywords: ["empty inbox", "zero screen", "penguin", "animation", "confetti"] },
  { page: "inbox", label: "Floe", keywords: ["floe mode", "single column", "focus mode"] },
  { page: "inbox", label: "Start in Floe mode", desc: "One calm column, no sidebar", keywords: ["focus", "zen", "full width", "minimal", "distraction", "floe"] },

  // Sync
  { page: "sync", label: "Download in full", desc: "How much mail is kept with bodies", keywords: ["sync window", "months", "offline", "history", "bodies"] },
  { page: "sync", label: "Older mail", keywords: ["headers", "metadata", "archive"] },
  { page: "sync", label: "Downloaded", keywords: ["coverage", "progress", "storage"] },
  { page: "sync", label: "Free up space", keywords: ["disk", "storage", "clean up", "delete bodies", "size"] },

  // Calendar
  { page: "calendar", label: "Google Calendar", desc: "Connect each account's calendar", keywords: ["connect calendar", "events"] },
  { page: "calendar", label: "Answer invitations from Penguin", keywords: ["rsvp", "accept", "decline", "maybe", "invites"] },
  { page: "calendar", label: "Connect calendar when adding accounts", keywords: ["workspace admin", "sign in"] },
  { page: "calendar", label: "Keep past events", keywords: ["history", "last meeting"] },
  { page: "calendar", label: "Sync upcoming events", keywords: ["recurring", "future", "ahead"] },
  { page: "calendar", label: "Next up in the status bar", keywords: ["meeting reminder", "join", "zoom", "meet", "upcoming"] },

  // Compose
  { page: "compose", label: "Writing font", keywords: ["font", "typeface", "serif", "sans"] },
  { page: "compose", label: "Size", desc: "Composer font size", keywords: ["font size", "text size", "bigger", "smaller"] },
  { page: "compose", label: "Preview", keywords: ["font preview"] },
  { page: "compose", label: "Reply from the account it came to", keywords: ["from address", "send as", "alias", "reply from"] },
  { page: "compose", label: "Undo send", desc: "How long a sent message waits", keywords: ["unsend", "cancel send", "delay", "recall", "off"] },
  { page: "compose", label: "Morning send time", desc: "When Send later's morning choices go out", keywords: ["send later", "schedule", "scheduled send", "tomorrow morning", "8 am"] },
  { page: "compose", label: "Instant replies", desc: "One-liners offered when you reply", keywords: ["quick replies", "canned responses", "one-liners", "smart reply", "short replies"] },
  { page: "compose", label: "Snippets", desc: "Reusable text, typed with ; or ⌘;", keywords: ["templates", "canned responses", "text expansion", "variables", "attachments", "cc"] },

  // Signatures
  { page: "signatures", label: "New signature", find: ".sn-settings", desc: "Rich text, one default per account", keywords: ["edit signature", "sig", "create"] },
  { page: "signatures", label: "Default signature per account", keywords: ["default", "per account"] },
  { page: "signatures", label: "Add to new messages", keywords: ["signature", "insert"] },
  { page: "signatures", label: "Add to replies", keywords: ["signature", "insert"] },
  { page: "signatures", label: "Add to forwards", keywords: ["signature", "insert"] },
  { page: "signatures", label: "\"-- \" separator", keywords: ["dashes", "delimiter", "signature separator"] },

  // Privacy
  { page: "privacy", label: "Remote images", desc: "Ask, always or never", keywords: ["load images", "pictures", "block images", "tracking"] },
  { page: "privacy", label: "Always load images from", keywords: ["trusted senders", "allow images"] },
  {
    page: "privacy",
    label: "Block tracking pixels",
    desc: "Remove trackers even when images load",
    keywords: ["tracking pixels", "spy pixels", "trackers", "tracker blocking", "open tracking", "read receipts", "web beacons"],
  },
  {
    page: "privacy",
    label: "Ask for read receipts",
    desc: "Request a receipt on mail you send; their app decides",
    keywords: ["read receipts", "read status", "opened", "seen", "tracking", "return receipt", "disposition notification", "mdn"],
  },
  {
    page: "privacy",
    label: "Remove tracking from links",
    desc: "Strip identifiers like mc_eid and fbclid from links",
    keywords: ["link tracking", "utm", "fbclid", "gclid", "clean links", "url parameters", "query parameters", "click tracking"],
  },
  { page: "privacy", label: "Sender photos", desc: "Where sender photos come from", keywords: ["avatars", "logos"] },
  { page: "privacy", label: "Contact photos (Google)", keywords: ["contacts", "people", "avatars"] },
  { page: "privacy", label: "Brand logos (BIMI)", keywords: ["bimi", "logos", "verified"] },
  { page: "privacy", label: "Company icons", keywords: ["favicons", "logos"] },
  { page: "privacy", label: "Gravatar", keywords: ["avatars"] },
  { page: "privacy", label: "Photo cache", keywords: ["clear cache", "images"] },
  { page: "privacy", label: "Show unsubscribe button", keywords: ["unsubscribe", "mailing list", "newsletters", "opt out"] },

  // Search
  { page: "search", label: "Search by meaning", desc: "Finds mail that says the same thing in other words", keywords: ["semantic", "embeddings", "similar", "meaning", "ai", "vector", "model", "indexing"] },
  { page: "search", label: "Search index", find: ".index-size", desc: "Size and location on this Mac", keywords: ["database", "size", "show in finder", "fts"] },
  { page: "search", label: "Optimize index", find: "[data-setting='optimize-index']", keywords: ["vacuum", "compact", "speed up", "rebuild", "slow search"] },
  { page: "search", label: "Recent searches", keywords: ["history", "clear", "ask", "questions"] },

  // AI
  { page: "ai", label: "Read questions with Apple Intelligence", desc: "Ask, when its grammar can't read a question", keywords: ["ask", "answers in search", "questions", "understand", "apple intelligence", "on device", "foundation models"] },
  { page: "ai", label: "Write with AI", desc: "Draft and rewrite in the composer, on this Mac", keywords: ["write", "generate", "draft", "rewrite", "shorter", "friendlier", "formal", "grammar", "proofread", "apple intelligence"] },
  { page: "ai", label: "Suggested replies", desc: "Three short replies when you reply", keywords: ["smart reply", "instant reply", "quick replies", "suggestions", "apple intelligence"] },
  { page: "ai", label: "Thread summaries", desc: "Apple Intelligence, on this Mac", keywords: ["summary", "summarize", "tldr", "apple intelligence", "on device", "foundation models"] },

  // Views (features/smart): one row per smart view, plus order and pinned searches.
  { page: "views", label: "Receipts", desc: "Orders and receipts, this month's total", keywords: ["orders", "purchases", "spending", "money", "shopping", "smart view"] },
  { page: "views", label: "Travel", desc: "Flights, hotels, rental cars, trains", keywords: ["flights", "trips", "hotels", "itinerary", "smart view"] },
  { page: "views", label: "Packages", desc: "Shipments, carrier and status", keywords: ["deliveries", "shipping", "tracking", "parcels", "smart view"] },
  { page: "views", label: "Bills", desc: "Invoices by due date, overdue highlighted", keywords: ["invoices", "due", "payments", "overdue", "smart view"] },
  { page: "views", label: "Reservations", desc: "Restaurants, events and tickets", keywords: ["restaurants", "tickets", "events", "bookings", "smart view"] },
  { page: "views", label: "Invites", desc: "Invitations waiting for your answer", keywords: ["invitations", "rsvp", "meetings", "smart view"] },
  { page: "views", label: "Codes", desc: "Verification codes from the last two days", keywords: ["verification", "otp", "2fa", "one-time", "smart view"] },
  { page: "views", label: "Files", desc: "Conversations with attachments", keywords: ["attachments", "pdf", "images", "documents", "spreadsheets", "smart view"] },
  { page: "views", label: "Newsletters", desc: "A reading list of newsletters in your inbox", keywords: ["reading list", "bulk", "mailing lists", "smart view"] },
  { page: "views", label: "Subscriptions", desc: "Recurring charges found in receipts", keywords: ["recurring", "monthly", "yearly", "plans", "memberships", "smart view"] },
  { page: "views", label: "People you know", desc: "Inbox mail from people you've written to", keywords: ["vip", "contacts", "known senders", "smart view"] },
  { page: "views", label: "Sidebar order", find: "[data-setting='views-order']", keywords: ["reorder", "drag", "arrange", "sidebar views", "move"] },
  { page: "views", label: "Pinned searches", find: "[data-setting='pin-search']", keywords: ["saved search", "pin", "custom view", "sidebar search"] },

  // Rules
  { page: "rules", label: "New rule", find: "[data-setting='new-rule']", desc: "Runs on arrival, on a schedule or by hand", keywords: ["create rule", "add rule", "filter", "test mode"] },
  { page: "rules", label: "Auto labels", find: "[data-setting='auto-labels']", desc: "Label and archive mail as it arrives", keywords: ["auto archive", "marketing", "pitch", "cold email", "social", "notifications", "receipts", "skip inbox", "filters"] },

  // Keyboard
  { page: "keyboard", label: "Shortcuts", find: ".shortcuts-link", desc: "The keyboard cheat sheet", keywords: ["hotkeys", "keybindings", "cheat sheet", "keys"] },
  {
    page: "keyboard",
    label: "Shortcut coach",
    desc: "Show the key after you click something that has one",
    keywords: ["learn shortcuts", "tips", "hints", "teach", "mouse", "key promoter"],
  },

  // Developer
  { page: "diagnostics", label: "Demo mode", desc: "Fictional accounts and mail", keywords: ["demo", "screenshots", "fake", "sample data", "fictional"] },
  { page: "diagnostics", label: "Gmail quota (units per minute per account)", keywords: ["quota", "rate limit", "api", "throttle", "google cloud"] },
  { page: "diagnostics", label: "AI tools (MCP)", keywords: ["mcp", "model context protocol", "claude"] },
  { page: "diagnostics", label: "Enable MCP server", keywords: ["mcp", "claude", "ai tools"] },
  { page: "diagnostics", label: "Command-line tool: penguin", keywords: ["cli", "terminal", "shell"] },
  { page: "diagnostics", label: "Rule hooks", keywords: ["webhooks", "scripts", "programs"] },
  { page: "diagnostics", label: "Allow hooks", keywords: ["webhooks", "scripts", "programs"] },
  { page: "diagnostics", label: "Diagnostics", desc: "Counts, sizes and sync state", keywords: ["copy diagnostics", "stats"] },
  { page: "diagnostics", label: "Accounts", desc: "Sync state and history per account", keywords: ["sync status", "errors"] },
  { page: "diagnostics", label: "Locations", keywords: ["data folder", "paths", "logs", "finder", "log viewer"] },
  { page: "diagnostics", label: "Tables and indexes", keywords: ["database", "sqlite", "size"] },

  // What's new
  { page: "whatsnew", label: "Updates", keywords: ["update", "auto update", "check for updates", "version", "restart"] },
  { page: "whatsnew", label: "Debug info", keywords: ["bug report", "copy", "support", "problems"] },

  // About
  { page: "about", label: "Made by Justin Gluska", keywords: ["author", "credits", "x", "twitter", "contact"] },
  { page: "about", label: "Source available", keywords: ["github", "code", "source", "license", "open source", "polyform"] },
];

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/** Lowercase, accents and punctuation folded to spaces: "Don't" → "don t". */
export function normalize(s: string): string {
  return s
    .toLowerCase()
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/['’`]/g, "")
    .replace(/[^a-z0-9⌘]+/g, " ")
    .trim();
}

function words(s: string): string[] {
  const n = normalize(s);
  return n ? n.split(" ") : [];
}

/** Best weight at which `token` starts a word of any of `fields` (0 = none). */
function tokenScore(token: string, fields: { words: string[]; weight: number }[]): number {
  let best = 0;
  for (const f of fields) {
    if (f.weight <= best) continue;
    if (f.words.some((w) => w.startsWith(token))) best = f.weight;
  }
  return best;
}

export interface SettingsHit {
  page: SettingsPage;
  /** The matching row, or null when the page itself matched. */
  entry: SettingEntry | null;
  score: number;
}

export interface SettingsResultGroup {
  page: SettingsPage;
  /** Whether the page itself matched (by its name or keywords). */
  pageMatched: boolean;
  hits: SettingsHit[];
}

/**
 * 0 when some word of the query matches nothing. Otherwise each word scores
 * its best field, plus a bonus when the whole query is the label, is one of
 * the keywords (how a row claims a common word: "dark" → Theme), or starts
 * the label.
 */
function scoreText(q: string, tokens: string[], label: string, keywords: string[], fields: { words: string[]; weight: number }[]): number {
  let score = 0;
  for (const t of tokens) {
    const s = tokenScore(t, fields);
    if (!s) return 0;
    score += s;
  }
  const nl = normalize(label);
  if (nl === q) score += 6;
  else if (keywords.some((k) => normalize(k) === q)) score += 3;
  else if (nl.startsWith(q)) score += 2;
  return score;
}

/**
 * Settings matching `query`, grouped by page. Every word of the query must
 * start a word of the row's label, keywords, description or page name.
 * Groups come in order of their best match; rows within a group by score,
 * then as they appear on the page.
 */
export function searchSettings(
  query: string,
  { index = SETTINGS_INDEX }: { index?: SettingEntry[] } = {},
): SettingsResultGroup[] {
  const q = normalize(query);
  if (!q) return [];
  const tokens = q.split(" ");
  const groups: (SettingsResultGroup & { best: number })[] = [];
  for (const page of SETTINGS_PAGES) {
    const pageWords = words(page.label);
    const pageScore = scoreText(q, tokens, page.label, page.keywords ?? [], [
      { words: pageWords, weight: 4 },
      { words: words((page.keywords ?? []).join(" ")), weight: 2 },
    ]);
    const hits: SettingsHit[] = [];
    for (const entry of index) {
      if (entry.page !== page.id) continue;
      const score = scoreText(q, tokens, entry.label, entry.keywords ?? [], [
        { words: words(entry.label), weight: 4 },
        { words: words((entry.keywords ?? []).join(" ")), weight: 3 },
        { words: words(entry.desc ?? ""), weight: 2 },
        { words: pageWords, weight: 1 },
      ]);
      // A word matched only by the page's name isn't a reason to list every row on it.
      if (score > tokens.length) hits.push({ page, entry, score });
    }
    if (!hits.length && !pageScore) continue;
    hits.sort((a, b) => b.score - a.score);
    groups.push({ page, pageMatched: pageScore > 0, hits, best: Math.max(pageScore, ...hits.map((h) => h.score)) });
  }
  groups.sort((a, b) => b.best - a.best);
  return groups.map(({ page, pageMatched, hits }) => ({ page, pageMatched, hits }));
}

/** The results as one list, in display order: each page, then its rows. */
export function flattenResults(groups: SettingsResultGroup[]): SettingsHit[] {
  const out: SettingsHit[] = [];
  for (const g of groups) {
    out.push({ page: g.page, entry: null, score: 0 });
    out.push(...g.hits);
  }
  return out;
}

/** Whether rendered label text (a `.setting-label` or `.st-sub`) is this entry's row. */
export function labelMatches(entry: SettingEntry, text: string): boolean {
  const want = normalize(entry.label);
  return !!want && normalize(text).startsWith(want);
}
