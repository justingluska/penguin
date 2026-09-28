// Mailing-list mail for the Unsubscribe button (features/unsubscribe).
// OWNER: unsubscribe. Fictional senders on .example domains. `unsubscribe`
// is set by hand to what penguin_core::unsubscribe::plan returns for the
// same headers/body, since the mock has no Rust:
//   one-click (List-Unsubscribe + List-Unsubscribe-Post, verified sender)
//   one-click unknown (stored before the Post header was kept → needsCheck)
//   mailto (verified → sent directly; unverified → composer only)
//   body link (no header; "Unsubscribe" link in the footer)
//   none ("unsubscribe" in prose, no link)
import type { Address, MessageView, UnsubscribeMethod, UnsubscribeOffer, UnsubscribeOutcome } from "../types";
import { mockMail } from "./mail";
import { mockTrackers } from "./trackers";
import type { MockHandler } from "./index";

const MIN = 60_000;
const now = Date.now();

interface Spec {
  accountId: string;
  me: string;
  id: string;
  from: Address;
  subject: string;
  paragraphs: string[];
  footer: string;
  minutesAgo: number;
  verified: boolean;
  offer: Omit<UnsubscribeOffer, "verified" | "unsubscribed"> | null;
  trackers?: number;
}

function view(s: Spec): MessageView {
  const html =
    `<!DOCTYPE html><html class="pg-html"><head><meta charset="utf-8"><style>` +
    `body{margin:0;background:#fff;color:#1f2328;font:15px/1.55 -apple-system,system-ui,sans-serif}` +
    `.wrap{max-width:560px;margin:0 auto;padding:28px 24px}h1{font-size:22px;margin:0 0 14px}` +
    `.foot{margin-top:28px;padding-top:14px;border-top:1px solid #e5e7eb;color:#6b7280;font-size:12px}.foot a{color:#6b7280}` +
    `</style></head><body><div class="pg-root"><div class="wrap"><h1>${s.subject}</h1>` +
    s.paragraphs.map((p) => `<p>${p}</p>`).join("") +
    `<div class="foot">${s.footer}</div></div></div></body></html>`;
  const text = s.paragraphs.join("\n\n").replace(/<[^>]+>/g, "");
  return {
    accountId: s.accountId,
    id: `${s.id}-m0`,
    threadId: s.id,
    date: now - s.minutesAgo * MIN,
    from: s.from,
    to: [{ name: "Sam Okafor", email: s.me }],
    cc: [],
    bcc: [],
    replyTo: [],
    subject: s.subject,
    snippet: text.slice(0, 140),
    bodyText: text,
    html,
    blockedRemoteImages: 0,
    trackersRemoved: s.trackers ?? 0,
    trackers: mockTrackers(s.trackers ?? 0),
    labelIds: ["INBOX", "CATEGORY_PROMOTIONS"],
    attachments: [],
    unread: true,
    starred: false,
    senderAuthenticated: s.verified,
    otp: null,
    unsubscribe: s.offer ? { ...s.offer, verified: s.verified, unsubscribed: null } : null,
  };
}

const SPECS: Spec[] = [
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-unsub-oneclick",
    from: { name: "Harbor Weekly", email: "digest@news.harborweekly.example" },
    subject: "Five harbors worth the detour this fall",
    paragraphs: [
      "This week: a ferry-only island with one café, the best tide-pool walk on the coast, and why October is the quiet month.",
      "Plus: readers' photos from the lighthouse loop.",
    ],
    footer: `You're receiving Harbor Weekly because you signed up at harborweekly.example. <a href="https://news.harborweekly.example/u/ab12">Unsubscribe</a>`,
    minutesAgo: 35,
    verified: true,
    trackers: 3,
    offer: { method: "oneClick", source: "header", domain: "news.harborweekly.example", mailto: null, needsCheck: false, linkUrl: null },
  },
  {
    accountId: "acc-northwind",
    me: "sam@northwind.example",
    id: "t-unsub-check",
    from: { name: "Lumen Labs", email: "updates@lumenlabs.example" },
    subject: "Lumen 4.2: faster exports and a new timeline",
    paragraphs: ["Exports are up to 3× faster, and the new timeline view groups work by week.", "Read the full changelog on our site."],
    footer: `Lumen Labs, 12 Example Row. <a href="https://mail.lumenlabs.example/prefs">Email preferences</a>`,
    minutesAgo: 80,
    verified: true,
    trackers: 1,
    // Stored before Penguin kept List-Unsubscribe-Post: the check finds one-click.
    offer: {
      method: "link",
      source: "header",
      domain: "mail.lumenlabs.example",
      mailto: null,
      needsCheck: true,
      linkUrl: "https://mail.lumenlabs.example/prefs/unsubscribe?u=8f3a2c61d0&list=product-news&src=header",
    },
  },
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-unsub-mailto",
    from: { name: "Fieldnotes Digest", email: "list@fieldnotes.example" },
    subject: "Fieldnotes #88: notebooks, pens and paper weights",
    paragraphs: ["A reader asked which paper handles fountain pens best. We tested nine.", "Also: the tiny stapler that fits in a pencil case."],
    footer: "To leave this list, reply with “unsubscribe” in the subject.",
    minutesAgo: 190,
    verified: true,
    offer: {
      method: "mailto",
      source: "header",
      domain: "fieldnotes.example",
      mailto: { to: "leave-88@fieldnotes.example", subject: "unsubscribe", body: "" },
      needsCheck: false,
      linkUrl: null,
    },
  },
  {
    accountId: "acc-harbor",
    me: "sam@harbor-labs.example",
    id: "t-unsub-body",
    from: { name: "Tidewater Outfitters", email: "hello@tidewater.example" },
    subject: "Last call: 30% off rain shells",
    paragraphs: ["Our lightest shell is back in three colors. Sale ends Sunday at midnight.", "Free returns on everything."],
    footer: `Tidewater Outfitters · 400 Example Pier. <a href="https://email.tidewater.example/opt-out?u=sam">Unsubscribe</a> · <a href="https://tidewater.example/stores">Find a store</a>`,
    minutesAgo: 260,
    verified: true,
    trackers: 5,
    offer: {
      method: "link",
      source: "body",
      domain: "email.tidewater.example",
      mailto: null,
      needsCheck: false,
      linkUrl: "https://email.tidewater.example/opt-out?u=sam",
    },
  },
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-unsub-unverified",
    from: { name: "Prize Center", email: "winner@prize-center.example" },
    subject: "You have (1) reward waiting",
    paragraphs: ["Claim your reward before it expires."],
    footer: "Reply to be removed.",
    minutesAgo: 400,
    verified: false,
    offer: {
      method: "mailto",
      source: "header",
      domain: "prize-center.example",
      mailto: { to: "remove@prize-center.example", subject: "remove me", body: "Please remove sam.okafor@gmail.example" },
      needsCheck: false,
      linkUrl: null,
    },
  },
  {
    accountId: "acc-northwind",
    me: "sam@northwind.example",
    id: "t-unsub-none",
    from: { name: "Priya Raman", email: "priya@northwind.example" },
    subject: "Newsletter plan for Q4",
    paragraphs: [
      "I drafted the Q4 newsletter plan. One open question: should the footer say “unsubscribe” or “manage preferences”? Let's decide Thursday.",
    ],
    footer: "Priya",
    minutesAgo: 55,
    verified: true,
    // "unsubscribe" in prose without a link: no button.
    offer: null,
  },
];

const views = new Map<string, MessageView>();
for (const s of SPECS) {
  const v = view(s);
  views.set(`${v.accountId}/${v.id}`, v);
  mockMail.addThread({ accountId: s.accountId, threadId: s.id, subject: s.subject, labelIds: v.labelIds, messages: [v] });
}

function find(accountId: string, messageId: string): MessageView {
  const v = views.get(`${accountId}/${messageId}`);
  if (!v?.unsubscribe) throw { code: "invalidInput", message: "This message has no unsubscribe option" };
  return v;
}

export const unsubscribeHandlers: Record<string, MockHandler> = {
  unsubscribe_check: ({ accountId, messageId }) => {
    const v = find(accountId, messageId);
    const offer = v.unsubscribe!;
    if (offer.needsCheck) {
      // The header fetch found List-Unsubscribe-Post: One-Click.
      Object.assign(offer, { method: "oneClick", needsCheck: false, linkUrl: null });
    }
    return { ...offer };
  },
  unsubscribe: async ({ accountId, messageId, method }: Record<string, any>): Promise<UnsubscribeOutcome> => {
    const v = find(accountId, messageId);
    const offer = v.unsubscribe!;
    const m = method as UnsubscribeMethod;
    if (m !== offer.method && m !== "link") throw { code: "invalidInput", message: "This message's unsubscribe option changed. Try again." };
    if (m === "mailto" && !offer.verified) throw { code: "invalidInput", message: "Couldn't verify this sender" };
    if (m === "oneClick") await new Promise((r) => setTimeout(r, 700));
    const record = { at: Date.now(), method: m };
    for (const x of views.values())
      if (x.accountId === accountId && x.from.email === v.from.email && x.unsubscribe) x.unsubscribe.unsubscribed = record;
    return { method: m, domain: offer.domain, sender: v.from.email, sentTo: m === "mailto" ? offer.mailto!.to : null, record };
  },
};
