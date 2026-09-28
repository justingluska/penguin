// Realistic HTML mail for judging "Dark email bodies" (features/message-body/
// darkBody.ts) in the mock: a receipt, a newsletter with a brand header and
// a hidden preheader, a message with its own prefers-color-scheme rules, one
// that is already dark, and an Outlook-style reply. Fictional senders on
// .example domains. The documents mimic penguin-render's output (BASE_CSS,
// the .pg-root wrapper, the pg-dark-css sheet), since the mock has no Rust.
import type { Address, MessageView } from "../types";
import { mockMail } from "./mail";

const MIN = 60_000;
const now = Date.now();

// Mirrors BASE_CSS in crates/penguin-render/src/lib.rs.
const BASE_CSS =
  "html{color-scheme:light;background:#fff;-webkit-text-size-adjust:100%;text-size-adjust:100%}" +
  'body{margin:0;background:#fff;color:#1b1c1f;font:14px/1.5 -apple-system,BlinkMacSystemFont,"Inter","Segoe UI",Helvetica,Arial,sans-serif;overflow-wrap:break-word;word-wrap:break-word}' +
  ".pg-root{display:flow-root;box-sizing:border-box;padding:20px 24px;overflow-x:auto;overflow-y:hidden}" +
  "img{max-width:100%;height:auto}table{max-width:100%}a{color:#0b63ce}pre{white-space:pre-wrap}" +
  "blockquote{margin:0 0 0 4px;padding-left:12px;border-left:2px solid #d7d7d7;color:#555}" +
  "html[data-pg-scheme=dark]{color-scheme:dark;background:#1c1c1e}" +
  ":where(html[data-pg-scheme=dark]) body{background:#1c1c1e;color:#e4e4e5}" +
  ":where(html[data-pg-scheme=dark]) a{color:#70b8ff}" +
  ":where(html[data-pg-scheme=dark]) blockquote{border-left-color:#48484a;color:#aeaeb2}";

// A sunset "photo" and a logo on a transparent background (dark wordmark).
const PHOTO =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAPAAAABkCAYAAAC4or3HAAAEJElEQVR42u3TaZIVRRSG4bMcF+AaWAY9z/OMooID4oA4ICqIKKKA7YCoy3IFVmUdw0tYPVy671BZmXkq3x/PHyKIW32+fEX/uqQAbJLqzwMFYJNUfxwoAJukenagAGyS6tm+ArBJqt/3FYBNUj3dVwA2SfV0TwHYJNVvewrAJql+3VMANhEwYDrgX3YVgE1S/byrAGyS6nBXAdgk1eGOArBJqp92FIBNUj3ZUQA2SfVkWwHYJNXjbQVgk1SPthWATQQMWA7Y/bilAGwS98OWArBJ3MMtBWCTuIebCsAmcd9vKgCbxD3YVAA2iXuwoQBsEvfdhgKwSdy3GwrAJgIGTAd8f10B2CTum3UFYJO4e+sKwCZx99YUgE3ivl5TADaJu7umAGwSd3dVAdgk7s6qArBJ3FerCsAmAgYsB1x+uaIAbJLyixUFYJOUt1cUgE1S3l5WADZJ+fmyAiHo34dD417DkfLWsgJtGSXaM2PmjmeS8taSAm3wEe9RxNzzRaT8bEkBn3yG2xcy9z1Byk+XFPClzXjriLlzjYBhKl4iPh3wJ4sKNBUy3jpi7q5SfryoQFNRAubuKuXNRQWaiBFvHXHmt5fy5oIC44oZ71HE+d5fyo8WFBhXEgFH+LtfevnCmUJ+h5Q3FhSj6xst0zskEXDE3c8NOcD3SHljXjG8waPlc4sU4j2KOP72Md6DlB/OKwYbebgMbpJUwIntH+otSPnBvOJ8Yw/X8bskFXCC+4d4CwTc8ngEbDtgH/G2+RakeH9O8WI+x+vifVIKOPX923oHUrw3p+jXyngdu1FSARvYv413IMX1OcVJrY7XoTslFbCR/X2/AymuzypOan+8MN/c9p3SCtjG9r63keLdWcWRUAP2Rgz4rW3cKqmAjW3vaxMprs0qngs9YG/EgN/p+15JBZzR/sdJcW1GMRNlvP4xw3yfr5ulFbDd/Zt8txTvzCjSCDjoo/F0tyTi7cD24367FG/PaO5yi7d+NB5ul0TAHdl+nO8X4r1AwBkG3JVNpPef3prOUs7x1g/Gwx19BznMd9fxdnT3Yf8Wqf/Dm9PZIWB/24cKt8n3d3EXOfEfrk5ng3BPPZaG94wV76C/p4sbHf+3UwFPBdf/keF/E35un0K4uZG+Ea9MtW6kRxXxt7OMuOF9iTdywL0R35hqRaOHFfn3swnYw52JN3LAz4ec9MrfA4v7+3lE7Gdz4o0YcG/I1ycba+2RRf79zkfsYfv/EG/EgHtDvjY5tiAPLeJvdz7iBtuzSSIBjzMkhyVi3kRCAf/vn8sT5+Kg3TRod96FkYDrQV+dqHHETCI+tvkouF2CASPTiF+ZGAk3I2AkF/HFgbgTASP1kC9d7MNdCBgAAQMEDICAARAwAAIGCBgAAQMgYICAARAwAAIGQMAAAQNI1b+MVwx0qndJ0QAAAABJRU5ErkJggg==";
const LOGO =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAHgAAAAgCAYAAADZubxIAAAAg0lEQVR42u3ZMQ6AIAxAUSZ3TutZPQ3OTKCJiW3fT3qBvqWB1iRJUoiu8xirsaWksKCLwIIuhAu5AC5kwFO992H25je4T5DBATZvgb/E3UUGB9hEBlaw6xkwYAEWYLmiQz5MAAbsJQsw4DLAfpMAu55d1HDLINtiUmhbSwptS5IkLbsBzuy/B2EJG3gAAAAASUVORK5CYII=";

function doc(css: string, body: string, opts: { dark?: string; rootStyle?: string } = {}): string {
  const dark = opts.dark !== undefined ? `<style class="pg-dark-css" media="not all">${opts.dark}</style>` : "";
  const rootStyle = opts.rootStyle ? ` style="${opts.rootStyle}"` : "";
  return (
    `<!DOCTYPE html><html class="pg-html"><head><meta charset="utf-8"><meta name="color-scheme" content="light">` +
    `<style>${BASE_CSS}</style><style>${css}</style>${dark}</head><body><div class="pg-root"${rootStyle}>${body}</div></body></html>`
  );
}

const RECEIPT = doc(
  ".card{background:#ffffff;border-radius:8px;border:1px solid #e5e7eb}.muted{color:#6b7280}.item td{border-bottom:1px solid #e5e7eb;padding:10px 0}" +
    ".btn{display:inline-block;background:#111111;color:#ffffff !important;text-decoration:none;padding:10px 18px;border-radius:6px;font-weight:600}",
  `<table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td align="center">
  <table role="presentation" class="card" width="560" cellpadding="0" cellspacing="0" style="margin:8px auto">
    <tr><td style="padding:24px 28px 8px"><img src="${LOGO}" width="120" height="32" alt="Brightside Coffee"></td></tr>
    <tr><td style="padding:8px 28px">
      <h1 style="font-size:22px;margin:0 0 6px;color:#111827">Thanks for your order, Sam</h1>
      <p class="muted" style="margin:0 0 18px">Order #10482 · September 24 · Pickup at Harbor St.</p>
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="font-size:14px;color:#374151">
        <tr class="item"><td>Oat flat white (large)</td><td align="right">$5.40</td></tr>
        <tr class="item"><td>Cardamom bun</td><td align="right">$4.25</td></tr>
        <tr class="item"><td>Single-origin beans, Huila 250 g</td><td align="right">$17.00</td></tr>
        <tr><td style="padding:10px 0" class="muted">Tax</td><td align="right" class="muted">$2.14</td></tr>
        <tr><td style="padding:6px 0;font-weight:700;color:#111827">Total</td><td align="right" style="font-weight:700;color:#111827">$28.79</td></tr>
      </table>
      <div style="background:#f3f4f6;border-radius:6px;padding:12px 14px;margin:18px 0;font-size:13px;color:#374151">
        Paid with Visa ending 4242. <span style="color:#15803d;font-weight:600">You earned 29 stars.</span>
      </div>
      <p style="margin:18px 0"><a class="btn" href="https://orders.brightside.example/10482">View order</a></p>
      <p style="font-size:13px">Questions about your order? <a href="https://help.brightside.example">Visit the help center</a>.</p>
    </td></tr>
    <tr><td style="padding:16px 28px 24px;border-top:1px solid #e5e7eb;font-size:12px" class="muted">
      Brightside Coffee Co., 14 Harbor St, Port Example. <a href="https://brightside.example/prefs" style="color:#6b7280">Email preferences</a>
    </td></tr>
  </table></td></tr></table>`,
  { rootStyle: "background-color:#f4f4f5;" },
);

const NEWSLETTER = doc(
  ".wrap{max-width:600px;margin:0 auto;background:#ffffff}.hdr{background:#1e3a8a;color:#ffffff;padding:22px 28px}" +
    ".hdr a{color:#bfdbfe}.sec{padding:20px 28px}.alt{background:#f8fafc}h2{font-size:18px;margin:0 0 8px;color:#0f172a}" +
    ".cta{display:inline-block;background:#e11d48;color:#ffffff;padding:11px 20px;border-radius:999px;text-decoration:none;font-weight:600}" +
    ".note{background:#fef3c7;border-left:4px solid #f59e0b;color:#92400e;padding:12px 16px;border-radius:4px}" +
    ".foot{padding:18px 28px;color:#6b7280;font-size:12px;text-align:center}.foot a{color:#6b7280}",
  `<div style="display:none;font-size:1px;color:#ffffff;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden">Tide tables, a lighthouse loop, and the café that only opens at low tide.</div>
  <div class="wrap">
    <div class="hdr"><div style="font-size:12px;letter-spacing:.08em;text-transform:uppercase;color:#93c5fd">The Tidewater Dispatch · Issue 112</div>
      <div style="font-size:26px;font-weight:700;margin-top:6px">Five harbors worth the detour</div>
      <div style="margin-top:6px">A weekly field guide for coastal walkers. <a href="https://tidewater.example/issue/112">Read online</a></div></div>
    <img src="${PHOTO}" width="600" alt="Sunset over the hills" style="display:block;width:100%">
    <div class="sec"><h2>This week on the coast</h2>
      <p>October is the quiet month: the ferries thin out, the light goes gold by four, and the tide pools belong to whoever shows up first.</p>
      <ul><li><b>Saltmarsh Point</b>: the boardwalk reopened after the storm.</li><li><b>Gull Island</b>: one café, cash only, open at low tide.</li><li><b>The lighthouse loop</b>: 6 km, bring a layer.</li></ul>
      <p><a href="https://tidewater.example/guides/harbors">See all five harbors →</a></p></div>
    <div class="sec alt"><h2>From the tide tables</h2>
      <table width="100%" cellpadding="6" cellspacing="0" style="border-collapse:collapse;font-size:13px">
        <tr style="background:#e2e8f0;color:#334155"><th align="left">Day</th><th align="left">Low</th><th align="left">High</th></tr>
        <tr><td style="border-bottom:1px solid #e2e8f0">Sat</td><td style="border-bottom:1px solid #e2e8f0">06:12</td><td style="border-bottom:1px solid #e2e8f0">12:31</td></tr>
        <tr><td>Sun</td><td>06:58</td><td>13:15</td></tr></table>
      <p class="note" style="margin-top:14px"><b>Heads up:</b> king tides on the 8th. Stay off the causeway after 11:00.</p></div>
    <div class="sec"><div style="background:linear-gradient(135deg,#e0f2fe,#fce7f3);border-radius:8px;padding:14px 16px;color:#334155;font-size:13px">
      <b style="color:#0f172a">Sponsored by Driftwood Maps.</b> Offline trail maps for the whole coast, now with tide overlays.</div></div>
    <div class="sec" style="text-align:center"><a class="cta" href="https://tidewater.example/join">Join a guided walk</a></div>
    <hr style="border:0;border-top:1px solid #e5e7eb;margin:0 28px">
    <div class="foot">You're receiving this because you subscribed at tidewater.example.<br><a href="https://tidewater.example/u/abc">Unsubscribe</a> · <a href="https://tidewater.example/prefs">Preferences</a></div>
  </div>`,
  { rootStyle: "background-color:#eef2f7;" },
);

// Ships its own dark design: penguin-render moves the
// @media (prefers-color-scheme: dark) block into the pg-dark-css sheet.
const DARK_AWARE = doc(
  ".box{max-width:560px;margin:0 auto;background:#ffffff;border:1px solid #e4e4e7;border-radius:12px;padding:28px}" +
    ".eyebrow{color:#7c3aed;font-weight:600;font-size:12px;text-transform:uppercase;letter-spacing:.06em}.lead{color:#3f3f46}" +
    ".pill{display:inline-block;background:#ede9fe;color:#5b21b6;border-radius:999px;padding:2px 10px;font-size:12px}",
  `<div class="box"><div class="eyebrow">Lumen Labs · Changelog</div>
    <h1 style="margin:8px 0 10px;font-size:24px">Lumen 4.2: faster exports and a new timeline</h1>
    <p class="lead">Exports are up to 3× faster, and the new timeline view groups your work by week. <span class="pill">New</span></p>
    <p class="lead">Keyboard users: <b>T</b> opens the timeline, <b>⇧E</b> exports the current view.</p>
    <p><a href="https://lumenlabs.example/changelog/4.2">Read the full changelog</a></p></div>`,
  {
    dark:
      "@media (min-width:0){.box{background:#18181b !important;border-color:#3f3f46 !important;color:#f4f4f5 !important}" +
      ".lead{color:#d4d4d8 !important}.eyebrow{color:#c4b5fd !important}.pill{background:#4c1d95 !important;color:#ede9fe !important}" +
      "a{color:#a78bfa !important}}",
    rootStyle: "background-color:#fafafa;",
  },
);

const ALREADY_DARK = doc(
  ".mix{max-width:560px;margin:0 auto}.card{background:#1f1f1f;border-radius:10px;padding:18px 20px;margin-bottom:12px}" +
    ".title{color:#ffffff;font-weight:700;font-size:16px}.sub{color:#a3a3a3;font-size:13px}.acc{color:#1ed760}",
  `<div class="mix"><p style="color:#ffffff;font-size:22px;font-weight:700;margin:4px 0 14px">Your Weekend Mix is ready</p>
    <div class="card"><div class="title">Slow Tide Sessions</div><div class="sub">Ambient · 42 min · <span class="acc">Saved</span></div></div>
    <div class="card"><div class="title">Harbor Lights, Vol. 3</div><div class="sub">Lo-fi · 58 min</div></div>
    <p style="color:#a3a3a3;font-size:12px">Nightjar Radio · <a href="https://nightjar.example/settings" style="color:#1ed760">Manage notifications</a></p></div>`,
  { rootStyle: "background-color:#121212;color:#ffffff;" },
);

const OUTLOOK = doc(
  "p.MsoNormal{margin:0}",
  `<div><p class="MsoNormal"><font face="Calibri" color="#1f497d">Hi Sam,</font></p><p class="MsoNormal">&nbsp;</p>
    <p class="MsoNormal"><font face="Calibri" color="#1f497d">Attached are the revised quotes. The <b>highlighted</b> rows changed since Tuesday:</font></p><p class="MsoNormal">&nbsp;</p>
    <table border="1" cellpadding="4" cellspacing="0" style="border-collapse:collapse;border-color:#a6a6a6">
      <tr bgcolor="#d9e1f2"><td><b>Item</b></td><td><b>Qty</b></td><td><b>Price</b></td></tr>
      <tr><td>Dock cleats</td><td>40</td><td>$312.00</td></tr>
      <tr bgcolor="#ffff00"><td>Mooring line, 20 m</td><td>12</td><td>$540.00</td></tr></table>
    <p class="MsoNormal">&nbsp;</p><p class="MsoNormal"><font face="Calibri" color="#1f497d">Thanks,<br>Marta Lindqvist<br><font color="#7f7f7f" size="1">Procurement · Harbor Supply Co.</font></font></p>
    <div style="border:none;border-top:solid #e1e1e1 1pt;padding:3pt 0 0 0;margin-top:12px"><p class="MsoNormal"><b>From:</b> Sam Okafor<br><b>Sent:</b> Tuesday<br><b>Subject:</b> Quotes</p></div>
    <blockquote>Could you resend the quote with the line lengths? Thanks!</blockquote></div>`,
);

interface Spec {
  accountId: string;
  me: string;
  id: string;
  from: Address;
  subject: string;
  snippet: string;
  html: string;
  minutesAgo: number;
  labels: string[];
}

const SPECS: Spec[] = [
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-dark-receipt",
    from: { name: "Brightside Coffee", email: "receipts@brightside.example" },
    subject: "Your Brightside order #10482",
    snippet: "Thanks for your order, Sam. Order #10482 · Pickup at Harbor St. Total $28.79",
    html: RECEIPT,
    minutesAgo: 12,
    labels: ["INBOX", "CATEGORY_UPDATES"],
  },
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-dark-newsletter",
    from: { name: "The Tidewater Dispatch", email: "dispatch@tidewater.example" },
    subject: "Five harbors worth the detour",
    snippet: "Tide tables, a lighthouse loop, and the café that only opens at low tide.",
    html: NEWSLETTER,
    minutesAgo: 18,
    labels: ["INBOX", "CATEGORY_PROMOTIONS"],
  },
  {
    accountId: "acc-northwind",
    me: "sam@northwind.example",
    id: "t-dark-aware",
    from: { name: "Lumen Labs", email: "changelog@lumenlabs.example" },
    subject: "Lumen 4.2 is out",
    snippet: "Exports are up to 3× faster, and the new timeline view groups your work by week.",
    html: DARK_AWARE,
    minutesAgo: 26,
    labels: ["INBOX", "CATEGORY_UPDATES"],
  },
  {
    accountId: "acc-personal",
    me: "sam.okafor@gmail.example",
    id: "t-dark-already",
    from: { name: "Nightjar Radio", email: "mixes@nightjar.example" },
    subject: "Your Weekend Mix is ready",
    snippet: "Slow Tide Sessions · Harbor Lights, Vol. 3",
    html: ALREADY_DARK,
    minutesAgo: 33,
    labels: ["INBOX", "CATEGORY_PROMOTIONS"],
  },
  {
    accountId: "acc-harbor",
    me: "sam@harbor-labs.example",
    id: "t-dark-outlook",
    from: { name: "Marta Lindqvist", email: "marta@harborsupply.example" },
    subject: "RE: Quotes",
    snippet: "Attached are the revised quotes. The highlighted rows changed since Tuesday.",
    html: OUTLOOK,
    minutesAgo: 41,
    labels: ["INBOX"],
  },
];

function view(s: Spec): MessageView {
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
    snippet: s.snippet,
    bodyText: s.snippet,
    html: s.html,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: s.labels,
    attachments: [],
    unread: true,
    starred: false,
    senderAuthenticated: true,
    otp: null,
    unsubscribe: null,
  };
}

for (const s of SPECS) {
  const v = view(s);
  mockMail.addThread({ accountId: s.accountId, threadId: s.id, subject: s.subject, labelIds: v.labelIds, messages: [v] });
}
