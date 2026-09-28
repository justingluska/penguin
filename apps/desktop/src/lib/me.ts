// "You": your photo (Settings → You) and your name, which is your Google
// profile name. The photo is a PNG the backend stores and hands back as a
// data: URL, fetched again only when `me.photo` (its hash) changes.
// Display-only: outgoing mail keeps each account's own From name.
// settings.json still carries the old `me` name/title/company/signOff fields;
// they are no longer edited or shown, so a stale value can't leak through.
import { useSyncExternalStore } from "react";
import { api } from "./api";
import { currentSettings, subscribeSettings } from "./settings";
import type { Account } from "./types";

let photoId: string | null = null;
let photoUrl: string | null = null;
let installed = false;
const subs = new Set<() => void>();

function sync() {
  const id = currentSettings().me.photo;
  if (id === photoId) return;
  photoId = id;
  if (!id) {
    photoUrl = null;
    subs.forEach((f) => f());
    return;
  }
  api.mePhoto().then(
    (url) => {
      if (photoId !== id) return; // superseded
      photoUrl = url;
      subs.forEach((f) => f());
    },
    (e) => console.warn("penguin: could not load your photo", e),
  );
}

function subscribe(cb: () => void): () => void {
  if (!installed) {
    installed = true;
    subscribeSettings(sync);
    sync();
  }
  subs.add(cb);
  return () => subs.delete(cb);
}

/** Your photo as a data: URL, or null (no photo, or not loaded yet). */
export function useMePhoto(): string | null {
  return useSyncExternalStore(subscribe, () => photoUrl);
}

/**
 * The name to show for you: the Google profile name of the account with that
 * address (when given), else the first account that has one, else null.
 */
export function meName(accounts: Account[], email?: string): string | null {
  const own = email ? accounts.find((a) => a.email.toLowerCase() === email.toLowerCase())?.displayName : null;
  return own || accounts.find((a) => a.displayName)?.displayName || null;
}
