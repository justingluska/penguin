// Attachment downloads take time, as they do on a real connection: the first
// fetch of each attachment streams penguin://attachment-progress before the
// command answers (src-tauri commands.rs reported_bytes); later ones come
// from the "cache" at once. ?mockSlowDownloads=1 slows it to a poor
// connection (~300 KB/s) to look at the progress ring.
import type { AttachmentProgressEvent } from "../types";
import { mockBackend, type MockHandler } from "./index";
import { findMockMessage } from "./mail";

/** Commands that fetch an attachment's bytes first. */
const FETCHING = ["save_attachment", "save_attachment_as", "preview_attachment", "quick_look_attachment", "prepare_attachment_drag", "copy_attachment_file"];

const cached = new Set<string>();
const running = new Map<string, Promise<void>>();

function rate(): number {
  const slow = typeof location !== "undefined" && new URLSearchParams(location.search).get("mockSlowDownloads") === "1";
  return slow ? 300 * 1024 : 8 * 1024 * 1024;
}

function download(accountId: string, messageId: string, attachmentId: string, size: number): Promise<void> {
  const key = `${accountId}\u0000${messageId}\u0000${attachmentId}`;
  if (cached.has(key)) return Promise.resolve();
  const inFlight = running.get(key);
  if (inFlight) return inFlight;
  const total = size > 0 ? size : null;
  const ms = Math.min(30_000, Math.max(300, ((size || 200_000) / rate()) * 1000));
  const p = new Promise<void>((resolve) => {
    const start = performance.now();
    const send = (received: number, done: boolean) =>
      mockBackend.emit("penguin://attachment-progress", { accountId, messageId, attachmentId, received, total, done } satisfies AttachmentProgressEvent);
    const tick = () => {
      const f = Math.min(1, (performance.now() - start) / ms);
      if (f >= 1) {
        send(size, true);
        cached.add(key);
        running.delete(key);
        resolve();
        return;
      }
      send(Math.floor(f * size), false);
      setTimeout(tick, 100);
    };
    tick();
  });
  running.set(key, p);
  return p;
}

export function withDownloads(handlers: Record<string, MockHandler>) {
  for (const cmd of FETCHING) {
    const base = handlers[cmd];
    if (!base) continue;
    handlers[cmd] = async (args) => {
      const m = findMockMessage(String(args.accountId), String(args.messageId));
      const a = m?.attachments.find((x) => x.id === args.attachmentId);
      if (a) {
        // A preview the backend answers from metadata (a type it can't show) fetches nothing.
        if (cmd === "preview_attachment") {
          const out = (await base(args)) as { kind: string; reason: string | null };
          if (out.kind === "unsupported" && out.reason !== "unreadable") return out;
          await download(String(args.accountId), String(args.messageId), a.id, a.size);
          return out;
        }
        await download(String(args.accountId), String(args.messageId), a.id, a.size);
      }
      return base(args);
    };
  }
}
