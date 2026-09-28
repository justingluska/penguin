// Display formatting helpers (dates, sizes, names). Pure functions, no state.
import type { Address } from "./types";

const DAY = 86_400_000;

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const weekdayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const monthDayFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullDateFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const monthYearFmt = new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" });

/** List-row time: "9:41 AM" today/yesterday, "Mon" this week, "Sep 2" this year, else "Sep 2, 2024". */
export function listTime(ms: number, now = Date.now()): string {
  const today = startOfDay(now);
  if (ms >= today - DAY) return timeFmt.format(ms);
  if (ms >= today - 6 * DAY) return weekdayFmt.format(ms);
  if (new Date(ms).getFullYear() === new Date(now).getFullYear()) return monthDayFmt.format(ms);
  return fullDateFmt.format(ms);
}

/** Message header time: "Today, 9:41 AM", "Yesterday, 6:12 PM", "Sep 15, 9:02 AM". */
export function messageTime(ms: number, now = Date.now()): string {
  const today = startOfDay(now);
  const t = timeFmt.format(ms);
  if (ms >= today) return `Today, ${t}`;
  if (ms >= today - DAY) return `Yesterday, ${t}`;
  if (new Date(ms).getFullYear() === new Date(now).getFullYear()) return `${monthDayFmt.format(ms)}, ${t}`;
  return `${fullDateFmt.format(ms)}, ${t}`;
}

/** Compact date for collapsed rows / side panels: "9:41 AM" today, else "Sep 15". */
export function shortDate(ms: number, now = Date.now()): string {
  if (ms >= startOfDay(now)) return timeFmt.format(ms);
  if (new Date(ms).getFullYear() === new Date(now).getFullYear()) return monthDayFmt.format(ms);
  return fullDateFmt.format(ms);
}

export function monthYear(ms: number): string {
  return monthYearFmt.format(ms);
}

export type DayGroup = "Today" | "Yesterday" | "Earlier";

export function dayGroup(ms: number, now = Date.now()): DayGroup {
  const today = startOfDay(now);
  if (ms >= today) return "Today";
  if (ms >= today - DAY) return "Yesterday";
  return "Earlier";
}

/** "just now", "2 min ago", "3 h ago". */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  return `${Math.round(h / 24)} d ago`;
}

/** Time left, humanized: "<1m", "~12m", "~1h 30m", "~2d". */
export function eta(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 1) return "<1m";
  if (m < 60) return `~${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `~${h}h ${m % 60}m` : `~${h}h`;
  return `~${Math.round(h / 24)}d`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export function num(n: number): string {
  return n.toLocaleString("en-US");
}

/** Display name, falling back to the local part of the address. */
export function displayName(a: Address): string {
  if (a.name && a.name.trim()) return a.name.trim();
  return a.email.split("@")[0];
}

export function firstName(a: Address): string {
  return displayName(a).split(/\s+/)[0];
}

export function initials(a: Address): string {
  const n = displayName(a);
  const parts = n.split(/[\s._-]+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

// Consumer mail providers: an account on one of these is labelled "Personal".
const CONSUMER = new Set(["gmail", "googlemail", "icloud", "me", "mac", "outlook", "hotmail", "live", "yahoo", "proton", "protonmail", "pm", "fastmail", "aol"]);

/** A personal mailbox provider (gmail.com, icloud.com…) rather than a company domain. */
export function isConsumerAddress(email: string): boolean {
  const domain = email.split("@")[1] ?? "";
  return CONSUMER.has(domain.split(".")[0].toLowerCase());
}

/**
 * Short account label for switchers and badges: "Northwind" for
 * sam@northwind.example, "Harbor Labs" for sam@harbor-labs.example, "Personal" for a
 * consumer address. Account.displayName is the person's name, not this.
 */
export function accountLabel(a: { email: string }): string {
  const domain = a.email.split("@")[1] ?? a.email;
  const first = domain.split(".")[0].toLowerCase();
  if (CONSUMER.has(first)) return "Personal";
  return first
    .split(/[-_]+/)
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

/** File-type tag for attachment cards: "PDF", "PNG", "XLS"… */
export function fileExt(filename: string): string {
  const m = /\.([a-z0-9]{1,5})$/i.exec(filename);
  if (!m) return "FILE";
  const e = m[1].toUpperCase();
  return e === "XLSX" ? "XLS" : e === "DOCX" ? "DOC" : e === "PPTX" ? "PPT" : e === "JPEG" ? "JPG" : e;
}

/** Tone class for a file type, matching the mockups (PDF red, images violet…). */
export function fileTone(filename: string, mimeType = ""): string {
  const e = fileExt(filename);
  if (e === "PDF") return "t-red";
  if (mimeType.startsWith("image/") || ["PNG", "JPG", "GIF", "HEIC", "WEBP", "SVG"].includes(e)) return "t-violet";
  if (["XLS", "CSV", "NUMBERS"].includes(e)) return "t-green";
  if (["KEY", "PPT", "DOC", "PAGES"].includes(e)) return "t-blue";
  if (["ZIP", "TAR", "GZ"].includes(e)) return "t-amber";
  return "t-gray";
}

// ---- Color → tone ----------------------------------------------------------
// Accounts and labels carry a hex color from the backend. The design system
// needs theme-aware tones instead (the same "violet" reads differently in dark
// and light), so the hex is mapped to the nearest tone by hue.

const TONES: [string, number][] = [
  ["red", 358],
  ["orange", 25],
  ["amber", 42],
  ["green", 150],
  ["cyan", 190],
  ["blue", 210],
  ["violet", 255],
];

const toneCache = new Map<string, string>();

export function toneForColor(hex: string | null | undefined): string {
  if (!hex) return "gray";
  const hit = toneCache.get(hex);
  if (hit) return hit;
  const m = /^#?([0-9a-f]{6})/i.exec(hex);
  let tone = "gray";
  if (m) {
    const v = parseInt(m[1], 16);
    const r = ((v >> 16) & 255) / 255;
    const g = ((v >> 8) & 255) / 255;
    const b = (v & 255) / 255;
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    const d = max - min;
    const l = (max + min) / 2;
    const sat = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
    if (sat >= 0.18) {
      let h = 0;
      if (max === r) h = 60 * (((g - b) / d) % 6);
      else if (max === g) h = 60 * ((b - r) / d + 2);
      else h = 60 * ((r - g) / d + 4);
      if (h < 0) h += 360;
      let best = Infinity;
      for (const [name, th] of TONES) {
        const dist = Math.min(Math.abs(h - th), 360 - Math.abs(h - th));
        if (dist < best) {
          best = dist;
          tone = name;
        }
      }
    }
  }
  toneCache.set(hex, tone);
  return tone;
}

/** Avatar tone for a person, stable by address. */
export function personTone(email: string): string {
  const names = ["violet", "cyan", "green", "orange", "blue", "amber", "red"];
  let h = 0;
  for (let i = 0; i < email.length; i++) h = (h * 31 + email.charCodeAt(i)) | 0;
  return names[Math.abs(h) % names.length];
}
