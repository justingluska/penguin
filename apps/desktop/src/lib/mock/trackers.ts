// Trackers found in mock messages, shaped like penguin_render's
// TrackerRemoved. Fictional .example hosts; the companies are the ones the
// real list would name for the same kind of pixel.
import type { MessageView, TrackerRemoved, TrackerStatus } from "../types";

type Found = Omit<TrackerRemoved, "status">;

const POOL: Found[] = [
  {
    host: "pixel.mailerlite.example",
    path: "/o/5f2c1a9e",
    kind: "knownTracker",
    company: "MailerLite",
    rule: "host pixel.mailerlite.example",
    hadQuery: true,
    count: 1,
  },
  {
    host: "img.newsletter.example",
    path: "/p/open.gif",
    kind: "hiddenImage",
    company: null,
    rule: "hidden image",
    hadQuery: true,
    count: 1,
  },
  {
    host: "links.harborweekly.example",
    path: "/wf/open",
    kind: "knownTracker",
    company: "SendGrid",
    rule: "path /wf/open",
    hadQuery: true,
    count: 1,
  },
  {
    host: "t.sidekickopen.example",
    path: "/e1t/o/5/aB3xQ",
    kind: "knownTracker",
    company: "HubSpot",
    rule: "path /e1t/o/",
    hadQuery: false,
    count: 1,
  },
  {
    host: "static.tidewater.example",
    path: "/spacer.gif",
    kind: "hiddenImage",
    company: null,
    rule: "hidden image",
    hadQuery: true,
    count: 2,
  },
];

/** `n` trackers (sum of counts) with one status, for `trackers` + `trackersRemoved`. */
export function mockTrackers(n: number, status: TrackerStatus = "removed"): TrackerRemoved[] {
  const out: TrackerRemoved[] = [];
  let left = n;
  for (const t of POOL) {
    if (left <= 0) break;
    const count = Math.min(t.count, left);
    out.push({ ...t, count, status });
    left -= count;
  }
  return out;
}

/** The privacy fields of a mock newsletter, as get_thread would render them under these settings. */
export function mockPrivacy(
  opts: { trackers: number; images: number; links: number; tracked: number },
  s: { blockTrackingPixels: boolean; stripLinkTracking: boolean },
  imagesLoaded: boolean,
): Pick<MessageView, "blockedRemoteImages" | "trackersRemoved" | "trackersAllowed" | "trackers" | "linksCleaned" | "linkParams" | "trackedLinks"> {
  const status: TrackerStatus = s.blockTrackingPixels ? "removed" : imagesLoaded ? "loaded" : "held";
  const held = !s.blockTrackingPixels && !imagesLoaded ? opts.trackers : 0;
  return {
    blockedRemoteImages: imagesLoaded ? 0 : opts.images + held,
    trackersRemoved: s.blockTrackingPixels ? opts.trackers : 0,
    trackersAllowed: s.blockTrackingPixels ? 0 : opts.trackers,
    trackers: mockTrackers(opts.trackers, status),
    linksCleaned: s.stripLinkTracking ? opts.links : 0,
    linkParams: s.stripLinkTracking && opts.links > 0 ? ["mc_eid", "_hsenc", "fbclid"].slice(0, Math.min(3, opts.links + 1)) : [],
    trackedLinks: opts.tracked,
  };
}
