// A conversation with pictures, for the image viewer (features/image-viewer)
// in the mock: a launch announcement with an inline hero (a cid: part, which
// penguin-render embeds as a data: URL), a linked banner, a small icon that
// stays undecorated, an image attachment, and a reply with an inline chart.
// Fictional people on .example domains; the pictures are drawn here.
// Open it with ?open=acc-harbor/t-penguin-launch.
import type { AttachmentMeta, MessageView } from "../types";
import { mockMail } from "./mail";
import type { MockHandler } from "./index";

const MIN = 60_000;
const now = Date.now();
const THREAD = "t-penguin-launch";
const ACCOUNT = "acc-harbor";
const ME = { name: "Sam Okafor", email: "sam@harbor-labs.example" };
const JUNIPER = { name: "Juniper Hale", email: "juniper@floe-studio.example" };
const THEO = { name: "Theo Laurent", email: "theo@harbor-labs.example" };

// Mirrors BASE_CSS in crates/penguin-render/src/lib.rs (as mock/darkBodies.ts does).
const BASE_CSS =
  "html{color-scheme:light;background:#fff;-webkit-text-size-adjust:100%;text-size-adjust:100%}" +
  'body{margin:0;background:#fff;color:#1b1c1f;font:14px/1.5 -apple-system,BlinkMacSystemFont,"Inter","Segoe UI",Helvetica,Arial,sans-serif;overflow-wrap:break-word;word-wrap:break-word}' +
  ".pg-root{display:flow-root;box-sizing:border-box;padding:20px 24px;overflow-x:auto;overflow-y:hidden}" +
  "img{max-width:100%;height:auto}table{max-width:100%}a{color:#0b63ce}pre{white-space:pre-wrap}";

type Draw = (g: CanvasRenderingContext2D, w: number, h: number) => void;

/** A picture drawn on a canvas, as penguin-render would embed it. Node (tests) gets a 1×1 PNG. */
function picture(w: number, h: number, type: "image/png" | "image/jpeg", draw: Draw): string {
  if (typeof document === "undefined") return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  draw(c.getContext("2d")!, w, h);
  return c.toDataURL(type, 0.86);
}

function penguin(g: CanvasRenderingContext2D, x: number, y: number, s: number) {
  g.save();
  g.translate(x, y);
  g.scale(s, s);
  g.fillStyle = "#15161a";
  g.beginPath();
  g.ellipse(0, 0, 60, 80, 0, 0, Math.PI * 2);
  g.fill();
  g.fillStyle = "#fbfbfd";
  g.beginPath();
  g.ellipse(0, 14, 40, 60, 0, 0, Math.PI * 2);
  g.fill();
  g.fillStyle = "#15161a";
  for (const ex of [-16, 16]) {
    g.beginPath();
    g.arc(ex, -34, 6, 0, Math.PI * 2);
    g.fill();
  }
  g.fillStyle = "#f5a524";
  g.beginPath();
  g.moveTo(-10, -22);
  g.lineTo(10, -22);
  g.lineTo(0, -8);
  g.closePath();
  g.fill();
  g.beginPath();
  g.ellipse(-22, 80, 18, 7, 0, 0, Math.PI * 2);
  g.ellipse(22, 80, 18, 7, 0, 0, Math.PI * 2);
  g.fill();
  g.restore();
}

const HERO = picture(1600, 900, "image/jpeg", (g, w, h) => {
  const sky = g.createLinearGradient(0, 0, w, h);
  sky.addColorStop(0, "#0f2a4a");
  sky.addColorStop(0.55, "#2a5d8f");
  sky.addColorStop(1, "#f0a868");
  g.fillStyle = sky;
  g.fillRect(0, 0, w, h);
  g.fillStyle = "rgba(255,255,255,.85)";
  for (let i = 0; i < 90; i++) g.fillRect((i * 173) % w, (i * 97) % (h * 0.45), 2, 2);
  g.fillStyle = "#e9f1f8";
  g.beginPath();
  g.moveTo(0, h * 0.78);
  g.bezierCurveTo(w * 0.3, h * 0.68, w * 0.6, h * 0.86, w, h * 0.72);
  g.lineTo(w, h);
  g.lineTo(0, h);
  g.fill();
  penguin(g, w * 0.72, h * 0.63, 2.1);
  penguin(g, w * 0.84, h * 0.7, 1.4);
  g.fillStyle = "#fff";
  g.font = "700 104px -apple-system, Inter, sans-serif";
  g.fillText("Penguin 2.0", 110, 300);
  g.font = "500 44px -apple-system, Inter, sans-serif";
  g.fillStyle = "rgba(255,255,255,.86)";
  g.fillText("Launching October 1 · faster search, calmer inbox", 114, 380);
});

const BANNER = picture(1200, 280, "image/png", (g, w, h) => {
  const bg = g.createLinearGradient(0, 0, w, 0);
  bg.addColorStop(0, "#5b3fd6");
  bg.addColorStop(1, "#1f7ae0");
  g.fillStyle = bg;
  g.fillRect(0, 0, w, h);
  penguin(g, 150, 150, 1.1);
  g.fillStyle = "#fff";
  g.font = "700 56px -apple-system, Inter, sans-serif";
  g.fillText("Join the launch stream →", 290, 140);
  g.font = "500 30px -apple-system, Inter, sans-serif";
  g.fillStyle = "rgba(255,255,255,.8)";
  g.fillText("Wed, Oct 1 · 10:00 AM PT · penguin.example/launch", 292, 196);
});

const ICON = picture(48, 48, "image/png", (g, w, h) => {
  g.fillStyle = "#1f7ae0";
  g.beginPath();
  g.arc(w / 2, h / 2, w / 2, 0, Math.PI * 2);
  g.fill();
  penguin(g, w / 2, h / 2 + 2, 0.18);
});

const CHART = picture(1000, 560, "image/png", (g, w, h) => {
  g.fillStyle = "#fff";
  g.fillRect(0, 0, w, h);
  g.fillStyle = "#1b1c1f";
  g.font = "600 30px -apple-system, Inter, sans-serif";
  g.fillText("Waitlist sign-ups, last 8 weeks", 48, 64);
  const vals = [120, 180, 260, 310, 420, 610, 880, 1240];
  const max = 1300;
  vals.forEach((v, i) => {
    const bh = ((h - 160) * v) / max;
    g.fillStyle = i === vals.length - 1 ? "#1f7ae0" : "#9cc2ee";
    g.fillRect(64 + i * 112, h - 60 - bh, 76, bh);
    g.fillStyle = "#6b6f76";
    g.font = "500 18px -apple-system, Inter, sans-serif";
    g.fillText(`W${i + 1}`, 86 + i * 112, h - 28);
  });
});

/** Decoded bytes of a base64 data: URL (what the inline part's size would be). */
function bytesOf(dataUrl: string): number {
  const b64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
  return Math.floor((b64.length * 3) / 4) - (b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0);
}

function inline(id: string, filename: string, dataUrl: string): AttachmentMeta {
  return { id, filename, mimeType: dataUrl.slice(5, dataUrl.indexOf(";")), size: bytesOf(dataUrl), contentId: `${id}@floe-studio.example`, inline: true };
}

function doc(body: string): string {
  return `<!DOCTYPE html><html class="pg-html"><head><meta charset="utf-8"><meta name="color-scheme" content="light"><style>${BASE_CSS}</style></head><body><div class="pg-root">${body}</div></body></html>`;
}

const LAUNCH_HTML = doc(
  `<table width="100%" cellpadding="0" cellspacing="0" style="max-width:600px;margin:0 auto"><tr><td>` +
    `<p style="margin:0 0 14px"><img src="${ICON}" width="24" height="24" alt="" style="vertical-align:middle;margin-right:8px"><b>Floe Studio</b> for Harbor Labs</p>` +
    `<p>Hi Sam,</p><p>Here's the final key art for the Penguin launch. The hero is what goes on the site and in the App Store listing; the banner links to the stream page.</p>` +
    `<p><img src="${HERO}" alt="Penguin 2.0 key art" width="600" style="width:100%;height:auto;border-radius:10px"></p>` +
    `<p><a href="https://penguin.example/launch"><img src="${BANNER}" alt="Join the launch stream" width="600" style="width:100%;height:auto;border-radius:10px"></a></p>` +
    `<p>A full-resolution screenshot of the new search view is attached, too. Let me know if the stars feel too busy.</p>` +
    `<p>Juniper<br><span style="color:#6b6f76">Floe Studio · design</span></p>` +
    `</td></tr></table>`,
);

const REPLY_HTML = doc(
  `<p>Love it. Here's where the waitlist is this morning, for the launch post:</p>` +
    `<p><img src="${CHART}" alt="Waitlist sign-ups" width="500" style="max-width:100%;height:auto;border:1px solid #e6e6e9;border-radius:8px"></p>` +
    `<p>Theo</p>`,
);

function view(i: number, from: typeof JUNIPER, html: string, minutesAgo: number, attachments: AttachmentMeta[], snippet: string): MessageView {
  return {
    accountId: ACCOUNT,
    id: `${THREAD}-m${i}`,
    threadId: THREAD,
    date: now - minutesAgo * MIN,
    from,
    to: [ME],
    cc: [],
    bcc: [],
    replyTo: [],
    subject: "Penguin launch: final key art",
    snippet,
    bodyText: snippet,
    html,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: ["INBOX"],
    attachments,
    unread: i === 1,
    starred: false,
    senderAuthenticated: true,
    otp: null,
    unsubscribe: null,
  };
}

const messages = [
  view(0, JUNIPER, LAUNCH_HTML, 52, [
    inline("hero", "penguin-2-key-art.jpg", HERO),
    inline("banner", "launch-stream-banner.png", BANNER),
    inline("icon", "floe-icon.png", ICON),
    { id: "shot", filename: "search-view-screenshot.png", mimeType: "image/png", size: 1_842_311, contentId: null, inline: false },
    { id: "brief", filename: "Launch_brief.pdf", mimeType: "application/pdf", size: 312_000, contentId: null, inline: false },
  ], "Here's the final key art for the Penguin launch. The hero is what goes on the site and in the App Store listing."),
  view(1, THEO, REPLY_HTML, 9, [inline("chart", "waitlist-chart.png", CHART)], "Love it. Here's where the waitlist is this morning, for the launch post."),
];

mockMail.addThread({ accountId: ACCOUNT, threadId: THREAD, subject: "Penguin launch: final key art", labelIds: ["INBOX"], messages });

export const imageHandlers: Record<string, MockHandler> = {
  // The mock has no network: remote pictures can't be fetched again.
  fetch_message_image: ({ url }) => {
    const src = String(url);
    const m = /^data:(image\/[a-z-]+);base64,/.exec(src);
    if (!m) throw { code: "network", message: "The mock can't fetch remote pictures" };
    return { mimeType: m[1], size: bytesOf(src), dataUrl: src };
  },
  save_message_image: ({ filename }) => `/Users/sam/Downloads/${String(filename)}`,
  // Like image_viewer::folder_name; nothing touches the disk.
  save_message_images: ({ messageId, items }) => {
    const subject = messages.find((m) => m.id === messageId)?.subject ?? "";
    const stem = subject.split(/\s+/).filter(Boolean).join(" ").replace(/^[. ]+/, "").replace(/[\u0000-\u001f/\\:]/g, "_").slice(0, 80).trimEnd() || "Email";
    const total = Array.isArray(items) ? items.length : 0;
    return { folder: `/Users/sam/Downloads/${stem} images`, saved: total, total };
  },
  prepare_image_drag: ({ filename }) => ({ path: `/Users/sam/Library/Caches/penguin/drag-out/mock/0/${String(filename)}`, name: String(filename) }),
  save_image_as: ({ filename }) => `/Users/sam/Downloads/${String(filename)}`,
  // The browser has no native drag; the UI doesn't offer one in the mock.
  start_file_drag: () => undefined,
  reveal_saved_path: () => undefined,
};
