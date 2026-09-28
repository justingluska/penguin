// What the privacy row and the privacy details dialog say about a message
// (pure; tested in tests/privacy.test.ts). Tracker entries come from
// penguin_render (TrackerRemoved): host + path only, never the query string.
// Link parameters are names only, never values.
import type { MessageView, TrackerRemoved } from "../../lib/types";

export interface TrackerCopy {
  /** "MailerLite", or the host when the company isn't known. */
  title: string;
  /** Short tag: "Known tracker" / "Hidden pixel". */
  tag: string;
  /** host + path, as shown (no query string). */
  address: string;
  /** Plain-English sentences: what it is and why it was caught. */
  lines: string[];
}

export function trackerCopy(t: TrackerRemoved): TrackerCopy {
  const lines: string[] = [];
  if (t.kind === "knownTracker") {
    lines.push(
      t.company
        ? `An open-tracking image from ${t.company}.`
        : "An open-tracking image from a known email-tracking service.",
    );
    lines.push(`It's on Penguin's list of trackers (${ruleText(t.rule)}).`);
  } else {
    lines.push("A tiny (3×3 pixels or smaller) or hidden image. An image nobody can see has only one job: reporting that the email was opened.");
  }
  if (t.count > 1) lines.push(`It appeared ${t.count} times in this message.`);
  if (t.status === "held") lines.push("Tracking pixel blocking is off, so it will load if you load images.");
  if (t.status === "loaded") lines.push("Tracking pixel blocking is off, so it loaded with the images.");
  return {
    title: t.kind === "knownTracker" && t.company ? t.company : t.host,
    tag: t.kind === "knownTracker" ? "Known tracker" : "Hidden pixel",
    address: t.host + t.path,
    lines,
  };
}

/** "host awstrack.me" → "matched by its host, awstrack.me". */
function ruleText(rule: string): string {
  if (rule.startsWith("host family ")) return `matched by its domain, ${rule.slice(12)}`;
  if (rule.startsWith("host ")) return `matched by its host, ${rule.slice(5)}`;
  if (rule.startsWith("path ")) return `matched by its path, ${rule.slice(5)}`;
  return rule;
}

/** Said once for the whole list when any address had a query string (shown as "?…"). */
export const QUERY_NOTE =
  "Where an address ends in “?…”, Penguin hid the rest. That part usually holds your email address or an ID unique to you, which is how an open is tied to you, and it's why these are never loaded.";

/** Trackers that were counted but not listed (the backend caps the list). `total` = removed + allowed. */
export function unlistedTrackers(total: number, list: TrackerRemoved[]): number {
  const listed = list.reduce((n, t) => n + t.count, 0);
  return Math.max(0, total - listed);
}

export function trackerLabel(n: number): string {
  return n === 1 ? "1 tracker removed" : `${n} trackers removed`;
}

/** Trackers found but not removed ("Block tracking pixels" off), and whether they loaded. */
export function allowedTrackers(m: MessageView): { count: number; loaded: boolean } {
  const count = m.trackersAllowed ?? 0;
  return { count, loaded: count > 0 && (m.trackers ?? []).some((t) => t.status === "loaded") };
}

export interface PrivacySummary {
  /** "2 trackers removed · 3 links cleaned". */
  label: string;
  /** "warn" when a tracker loaded (blocking is off and images are shown). */
  tone: "quiet" | "warn";
}

/**
 * The privacy row's summary of what was done to this message, or null when
 * there's nothing to say (the row then shows only for images or trailing
 * buttons). Held trackers are covered by the images count, so they're only
 * in the dialog.
 */
export function privacySummary(m: MessageView): PrivacySummary | null {
  const parts: string[] = [];
  const allowed = allowedTrackers(m);
  if (m.trackersRemoved > 0) parts.push(trackerLabel(m.trackersRemoved));
  if (allowed.loaded) parts.push(allowed.count === 1 ? "1 tracker loaded" : `${allowed.count} trackers loaded`);
  const cleaned = m.linksCleaned ?? 0;
  if (cleaned > 0) parts.push(cleaned === 1 ? "1 link cleaned" : `${cleaned} links cleaned`);
  if (parts.length === 0) return null;
  return { label: parts.join(" · "), tone: allowed.loaded ? "warn" : "quiet" };
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Message details' Privacy line: every count, zeros included for trackers and images. */
export function privacyFacts(m: MessageView): string {
  const allowed = allowedTrackers(m);
  const parts = [`${plural(m.trackersRemoved, "tracker", "trackers")} removed`];
  if (allowed.count > 0) parts.push(`${plural(allowed.count, "tracker", "trackers")} ${allowed.loaded ? "loaded" : "allowed"}`);
  parts.push(`${plural(m.blockedRemoteImages, "remote image", "remote images")} blocked`);
  if ((m.linksCleaned ?? 0) > 0) parts.push(`${plural(m.linksCleaned ?? 0, "link", "links")} cleaned`);
  return parts.join(" · ");
}

/** "mc_eid", "mc_eid and fbclid", "mc_eid, _hsenc and fbclid". */
export function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The dialog's paragraph about links, or null when there's nothing to say about them. */
export function linkCopy(m: MessageView, stripping: boolean): string[] | null {
  const cleaned = m.linksCleaned ?? 0;
  const tracked = m.trackedLinks ?? 0;
  const out: string[] = [];
  if (cleaned > 0) {
    const params = m.linkParams ?? [];
    out.push(
      `Penguin removed ${params.length ? listNames(params) : "tracking parameters"} from ${cleaned === 1 ? "1 link" : `${cleaned} links`}. ` +
        "Those parts of a link identify you or your click, so the site you open can tie the visit to this email. The links still go to the same pages.",
    );
  } else if (!stripping && tracked === 0) {
    return null;
  }
  if (tracked > 0) {
    out.push(
      `${tracked === 1 ? "1 link goes" : `${tracked} links go`} through the sender's click tracker first, so opening ${tracked === 1 ? "it" : "one"} tells the sender you clicked. ` +
        "The real destination is hidden inside the tracker's link, so Penguin can't remove that.",
    );
  }
  if (!stripping) out.push("Removing tracking from links is off (Settings → Privacy).");
  return out.length ? out : null;
}
