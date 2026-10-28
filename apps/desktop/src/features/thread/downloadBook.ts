// Bookkeeping for attachment downloads (no React, no IPC; attachmentProgress.ts
// feeds it): the backend's penguin://attachment-progress events plus the
// UI's own calls, so an attachment reads as busy from the click on, even
// before the first byte (or when the provider can't count, or the file
// comes from the cache).
import { bytes } from "../../lib/format.ts";
import type { AttachmentProgressEvent } from "../../lib/types.ts";

/** The file's bytes so far; `total` null when its size isn't known. */
export interface Received {
  received: number;
  total: number | null;
}

/** "waiting" = busy, nothing counted yet (a spinner, not a ring). */
export type AttachmentProgress = Received | "waiting";

interface Entry {
  /** UI calls for this attachment still running. */
  calls: number;
  /** Between the backend's first event and its `done`. */
  downloading: boolean;
  counted: Received | null;
}

export const downloadKey = (accountId: string, messageId: string, attachmentId: string) => `${accountId}\u0000${messageId}\u0000${attachmentId}`;

export class DownloadBook {
  private live = new Map<string, Entry>();

  private entry(key: string): Entry {
    let e = this.live.get(key);
    if (!e) this.live.set(key, (e = { calls: 0, downloading: false, counted: null }));
    return e;
  }

  private settle(key: string) {
    const e = this.live.get(key);
    if (e && e.calls === 0 && !e.downloading) this.live.delete(key);
  }

  /** One backend event. */
  apply(ev: AttachmentProgressEvent) {
    const key = downloadKey(ev.accountId, ev.messageId, ev.attachmentId);
    const e = this.entry(key);
    e.downloading = !ev.done;
    // Done with bytes = the whole file; a failure (none) keeps the last
    // count until the call that asked rejects.
    if (!ev.done || ev.received > 0) e.counted = { received: ev.received, total: ev.total ?? (ev.done ? ev.received : null) };
    this.settle(key);
  }

  /** A UI call for this attachment started. */
  begin(key: string) {
    this.entry(key).calls++;
  }

  /** It ended (either way): idle once no call or download is left. */
  end(key: string) {
    const e = this.live.get(key);
    if (e) {
      e.calls = Math.max(0, e.calls - 1);
      if (e.calls === 0) e.downloading = false;
    }
    this.settle(key);
  }

  /** Null when idle. */
  progress(key: string): AttachmentProgress | null {
    const e = this.live.get(key);
    if (!e) return null;
    return e.counted ?? "waiting";
  }
}

/** 0–1 when the total is known, else null (indeterminate). */
export function fractionOf(p: AttachmentProgress | null): number | null {
  if (!p || p === "waiting" || !p.total) return null;
  return Math.min(1, p.received / p.total);
}

/**
 * "42% · 1.2 MB of 3.4 MB" ("42% of 3.4 MB" short, for a card), "1.2 MB"
 * without a total, or "" while nothing is counted.
 */
export function progressText(p: AttachmentProgress | null, short = false): string {
  if (!p || p === "waiting") return "";
  if (!p.total) return p.received > 0 ? bytes(p.received) : "";
  const pct = Math.floor((Math.min(p.received, p.total) / p.total) * 100);
  return short ? `${pct}% of ${bytes(p.total)}` : `${pct}% · ${bytes(p.received)} of ${bytes(p.total)}`;
}
