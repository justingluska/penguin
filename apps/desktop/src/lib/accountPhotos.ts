// Each account's own Google profile photo (backend: src-tauri/src/avatars/account.rs).
//
// Components call useAccountPhoto(accountId) through AccountAvatar. Each
// account is asked once per session, off the render path; the answer (an
// `avatar:` URL, or null for "letter avatar only") lives here, so re-renders
// and remounts never reach the backend. Until it lands, and whenever the ask
// fails (offline, reconnect needed), the monogram stays. A failed account is
// asked again only after RETRY_MS, when something that shows it mounts or
// re-renders. When the photo cache is cleared or contacts re-sync
// (avatars-changed {all}), answered accounts are asked again in the
// background and keep their current photo until the new answer lands.
import { useEffect, useSyncExternalStore } from "react";
import { api, onAvatarsChanged } from "./api";

const RETRY_MS = 5 * 60_000;

const answers = new Map<string, string | null>();
const failedAt = new Map<string, number>();
const inFlight = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;

function notify() {
  version++;
  listeners.forEach((f) => f());
}

function ask(id: string, refresh = false) {
  if (inFlight.has(id) || (!refresh && answers.has(id))) return;
  const failed = failedAt.get(id);
  if (!refresh && failed !== undefined && Date.now() - failed < RETRY_MS) return;
  inFlight.add(id);
  api.accountPhoto(id).then(
    (url) => {
      inFlight.delete(id);
      failedAt.delete(id);
      answers.set(id, url);
      notify();
    },
    () => {
      // Decoration: keep the monogram (or the photo we had) and don't loop.
      inFlight.delete(id);
      failedAt.set(id, Date.now());
      notify();
    },
  );
}

let watching = false;
function watchCache() {
  if (watching) return;
  watching = true;
  void onAvatarsChanged((e) => {
    if (!e.all) return;
    failedAt.clear();
    for (const id of answers.keys()) ask(id, true);
  });
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** The account's photo URL once known, else null (show the monogram). */
export function useAccountPhoto(accountId: string | null | undefined): string | null {
  const v = useSyncExternalStore(subscribe, () => version);
  useEffect(() => {
    if (!accountId) return;
    watchCache();
    ask(accountId);
  }, [accountId, v]);
  return accountId ? (answers.get(accountId) ?? null) : null;
}
