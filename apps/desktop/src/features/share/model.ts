// Share links, the pure part (docs/SHARE-LINKS.md): the menu item, what to
// share for a picture or an attachment, lifetimes in words, the endpoint
// tidy-up for the settings form, and the check on a pending share handed
// over from another window. No DOM and no api, so node tests run it.
import type { MenuItem } from "../../components/menuModel.ts";
import type { AttachmentMeta, LinkLifetime, MessageView, ShareLinkConfigInput, ShareRequest } from "../../lib/types.ts";
import type { ViewerItem } from "../image-viewer/items.ts";

/** Setup documentation (bring-your-own storage, R2 as the example). */
export const SHARE_DOCS_URL = "https://github.com/justingluska/penguin/blob/main/docs/SHARE-LINKS.md";

export const LIFETIMES: { value: LinkLifetime; label: string }[] = [
  { value: "1h", label: "1 hour" },
  { value: "24h", label: "24 hours" },
  { value: "7d", label: "7 days" },
];

export function lifetimeLabel(l: LinkLifetime): string {
  return LIFETIMES.find((x) => x.value === l)?.label ?? l;
}

/** "expires in 24 hours" from the link's expiry (rounded to what it was made with). */
export function expiresIn(expiresAt: number, now: number): string {
  const hours = Math.max(0, Math.round((expiresAt - now) / 3_600_000));
  if (hours >= 48 && hours % 24 === 0) return `expires in ${hours / 24} days`;
  if (hours >= 1) return `expires in ${hours} ${hours === 1 ? "hour" : "hours"}`;
  const minutes = Math.max(1, Math.round((expiresAt - now) / 60_000));
  return `expires in ${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
}

/**
 * The picture and attachment menus' "Copy Share Link". Always offered; when
 * storage isn't set up yet it reads "Copy Share Link…" (more is needed) with
 * a quiet "Set up" hint, and choosing it opens the setup.
 */
export function shareMenuItem(ready: boolean, onSelect: () => void, disabled: boolean | string = false): MenuItem {
  return {
    label: ready ? "Copy Share Link" : "Copy Share Link…",
    text: "Copy Share Link",
    icon: "cloud",
    end: ready ? undefined : "Set up",
    disabled,
    onSelect,
  };
}

/** A file to share, as the UI knows it (the name and size for toasts). */
export interface ShareTarget {
  request: ShareRequest;
  name: string;
  /** Bytes, when known before the upload. */
  size: number | null;
}

export function attachmentShare(m: MessageView, a: AttachmentMeta): ShareTarget {
  return {
    request: { kind: "attachment", accountId: m.accountId, messageId: m.id, attachmentId: a.id },
    name: a.filename,
    size: a.size,
  };
}

/**
 * A picture: its attachment when it is one (or an embedded picture whose
 * inline part is known), else the body picture by its source, the bytes
 * Penguin already has (data:) or the address the body loaded (https).
 */
export function pictureShare(it: ViewerItem): ShareTarget {
  if (it.kind === "attachment") return attachmentShare(it.message, it.attachment);
  if (it.attachment) return { ...attachmentShare(it.message, it.attachment), name: it.attachment.filename || it.name };
  return {
    request: { kind: "picture", accountId: it.message.accountId, messageId: it.message.id, src: it.src, name: it.name },
    name: it.name,
    size: it.size,
  };
}

const MAX_SRC = 30 * 1024 * 1024;

/** A pending share handed over from a conversation window (bus message): checked, never trusted. */
export function asShareTarget(x: unknown): ShareTarget | null {
  if (!x || typeof x !== "object") return null;
  const t = x as Record<string, unknown>;
  const r = t.request as Record<string, unknown> | undefined;
  if (!r || typeof r !== "object" || typeof t.name !== "string" || t.name.length > 1000) return null;
  const str = (v: unknown, max = 1000) => typeof v === "string" && v.length > 0 && v.length <= max;
  const size = typeof t.size === "number" && Number.isFinite(t.size) && t.size >= 0 ? t.size : null;
  if (r.kind === "attachment" && str(r.accountId) && str(r.messageId) && str(r.attachmentId)) {
    return { request: { kind: "attachment", accountId: r.accountId as string, messageId: r.messageId as string, attachmentId: r.attachmentId as string }, name: t.name, size };
  }
  if (r.kind === "picture" && str(r.accountId) && str(r.messageId) && str(r.src, MAX_SRC) && typeof r.name === "string" && /^(data:image\/|https:\/\/)/i.test(r.src as string)) {
    return { request: { kind: "picture", accountId: r.accountId as string, messageId: r.messageId as string, src: r.src as string, name: r.name }, name: t.name, size };
  }
  return null;
}

/**
 * The endpoint as typed, tidied: trimmed, no trailing slash, and an R2
 * "S3 API" address copied with its bucket (https://<id>.r2.cloudflarestorage.com/<bucket>)
 * split into endpoint and bucket when the bucket field is empty or the same.
 */
export function tidyEndpoint(endpoint: string, bucket: string): { endpoint: string; bucket: string } {
  const e = endpoint.trim();
  let url: URL;
  try {
    url = new URL(e);
  } catch {
    return { endpoint: e, bucket };
  }
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts.length === 1 && !url.search && !url.hash && (!bucket.trim() || bucket.trim() === parts[0])) {
    return { endpoint: `${url.protocol}//${url.host}`, bucket: parts[0] };
  }
  return { endpoint: parts.length === 0 && !url.search && !url.hash ? `${url.protocol}//${url.host}` : e, bucket };
}

/** The storage fields every save and test needs (the rest have defaults). */
export function formMissing(f: ShareLinkConfigInput, hasSecret: boolean): string | null {
  if (!f.endpoint.trim()) return "Enter the endpoint";
  if (!f.bucket.trim()) return "Enter the bucket";
  if (!f.accessKeyId.trim()) return "Enter the access key ID";
  if (!hasSecret && !f.secretAccessKey?.trim()) return "Enter the secret access key";
  return null;
}
