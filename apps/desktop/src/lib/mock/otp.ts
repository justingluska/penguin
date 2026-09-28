// Verification-code mail for the mock inbox (features/otp). OWNER: otp.
// Fictional senders on .example domains. `otp` is set by hand to what
// penguin-core's detector (otp.rs) returns for the same text, since the mock
// has no Rust.
import type { Address, MessageView, Otp } from "../types";
import { mockMail } from "./mail";

const MIN = 60_000;
const now = Date.now();

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

interface Spec {
  accountId: string;
  me: string;
  id: string;
  from: Address;
  subject: string;
  body: string;
  minutesAgo: number;
  otp: Otp | null;
  verified: boolean;
  unread?: boolean;
}

function view(s: Spec): MessageView {
  const date = now - s.minutesAgo * MIN;
  return {
    accountId: s.accountId,
    id: `${s.id}-m0`,
    threadId: s.id,
    date,
    from: s.from,
    to: [{ name: "Sam Okafor", email: s.me }],
    cc: [],
    bcc: [],
    replyTo: [],
    subject: s.subject,
    snippet: s.body.slice(0, 140),
    bodyText: s.body,
    html:
      `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}` +
      `body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}</style></head><body>` +
      s.body.split("\n\n").map((p) => `<p>${esc(p)}</p>`).join("") +
      `</body></html>`,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: ["INBOX", "IMPORTANT", "CATEGORY_UPDATES"],
    attachments: [],
    unread: s.unread ?? true,
    starred: false,
    senderAuthenticated: s.verified,
    otp: s.otp ? { ...s.otp, date } : null,
  };
}

const code = (c: string, verified = true): Otp => ({ kind: "code", code: c, verified, date: 0 });

const SPECS: Spec[] = [
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-otp-rydeo",
    from: { name: "Rydeo", email: "admin@rydeo.example" },
    subject: "0357",
    body: "A one-time Rydeo code has been created for you.\n\nEnter 0357 in the app to finish signing in. It expires in 10 minutes. If you didn't request it, ignore this email.",
    minutesAgo: 2,
    verified: true,
    otp: code("0357"),
  },
  {
    accountId: "acc-northwind",
    me: "sam@northwind.example",
    id: "t-otp-glasswing",
    from: { name: "Glasswing Accounts", email: "no-reply@accounts.glasswing.example" },
    subject: "G-482913 is your Glasswing verification code",
    body: "Use this code to verify it's you.\n\nG-482913\n\nDon't share this code with anyone. Glasswing will never ask you for it.",
    minutesAgo: 7,
    verified: true,
    otp: code("G-482913"),
  },
  {
    accountId: "acc-harbor",
    me: "sam@harbor-labs.example",
    id: "t-otp-bank",
    from: { name: "Harborline Bank", email: "alerts@harborline-bank.example" },
    subject: "Your Harborline one-time passcode",
    body: "Do not share this code. Your one-time passcode is 40718325. It expires in 10 minutes.\n\nAccount ending 4412 · If you didn't try to sign in, call us.",
    minutesAgo: 46,
    verified: true,
    unread: false,
    otp: code("40718325"),
  },
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-otp-unverified",
    from: { name: "Account Security", email: "secure-login@acc0unt-verify.example" },
    subject: "Your verification code",
    body: "We noticed a sign-in attempt. Your verification code is 918273. Enter it within 5 minutes to keep your account active.",
    minutesAgo: 4,
    verified: false,
    otp: code("918273", false),
  },
  {
    accountId: "acc-northwind",
    me: "sam@northwind.example",
    id: "t-otp-link",
    from: { name: "Quanta", email: "login@quanta.example" },
    subject: "Sign in to Quanta",
    body: "Click the button below to sign in to Quanta. This link expires in 15 minutes and can only be used once.\n\nIf you didn't request this email, you can safely ignore it.",
    minutesAgo: 12,
    verified: true,
    otp: { kind: "link", code: null, verified: true, date: 0 },
  },
  // Negative: a number next to "invoice" is not a code.
  {
    accountId: "acc-harbor",
    me: "sam@harbor-labs.example",
    id: "t-otp-invoice",
    from: { name: "Ledgerly", email: "billing@ledgerly.example" },
    subject: "Invoice 20931 from Ledgerly",
    body: "Invoice number: 20931. Amount due $1,250.00 by 10/15. Pay online or reply with questions.",
    minutesAgo: 9,
    verified: true,
    unread: false,
    otp: null,
  },
];

for (const s of SPECS) {
  const v = view(s);
  mockMail.addThread({ accountId: s.accountId, threadId: s.id, subject: s.subject, labelIds: v.labelIds, messages: [v] });
}
