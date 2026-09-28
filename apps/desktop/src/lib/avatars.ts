// Sender photos and logos (backend: src-tauri/src/avatars/).
//
// Components call useAvatar(email) from the Avatar primitive. Only mounted
// avatars ask, and the list is virtualized, so only visible rows do.
// Requests made in the same frame go out as one avatar_lookup call, which
// answers from local state only. The backend resolves unknown senders in the
// background and names them in penguin://avatars-changed; we then ask again
// for the ones still on screen.
//
// Images are `avatar://` URLs with immutable cache headers, so WebKit keeps
// them decoded across row remounts. `loadedUrls` lets a remounted avatar skip
// the fade-in for an image that has already been shown once.
import { useEffect, useSyncExternalStore } from "react";
import { api, onAvatarsChanged } from "./api";
import { currentSettings, subscribeSettings, useSetting } from "./settings";
import type { AvatarInfo, AvatarPlacement, AvatarRequest, SenderPhotos } from "./types";

const BATCH_MAX = 200;

/** key → answer; absent = never asked (or invalidated). */
const answers = new Map<string, AvatarInfo>();
/** key → listeners of mounted avatars. */
const listeners = new Map<string, Set<() => void>>();
const queued = new Map<string, AvatarRequest>();
const inFlight = new Set<string>();
/** Invalidated while in flight: ask again when the answer lands. */
const stale = new Set<string>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;

/** URLs that finished loading at least once this session (skip the fade). */
export const loadedUrls = new Set<string>();

function keyOf(email: string, authenticated: boolean | null): string {
  return `${email.trim().toLowerCase()}|${authenticated === null ? "-" : authenticated ? "1" : "0"}`;
}

function notify(key: string) {
  listeners.get(key)?.forEach((f) => f());
}

function schedule() {
  if (flushTimer !== null) return;
  // One frame: every row that mounts in this render pass joins the batch.
  flushTimer = setTimeout(flush, 16);
}

function flush() {
  flushTimer = null;
  const batch = [...queued.entries()].slice(0, BATCH_MAX);
  if (batch.length === 0) return;
  for (const [k] of batch) {
    queued.delete(k);
    inFlight.add(k);
  }
  if (queued.size) schedule();
  api.avatarLookup(batch.map(([, r]) => r)).then(
    (infos) => {
      batch.forEach(([k, req], i) => {
        inFlight.delete(k);
        const info = infos[i];
        if (info) answers.set(k, info);
        notify(k);
        if (stale.delete(k) && listeners.has(k)) request(k, req, true);
      });
    },
    (e) => {
      // Avatars are decoration: keep the monograms and don't retry in a loop.
      console.warn("penguin: avatar lookup failed", e);
      batch.forEach(([k]) => {
        inFlight.delete(k);
        stale.delete(k);
        answers.set(k, { email: k.split("|")[0], kind: null, url: null });
      });
    },
  );
}

/** `refresh`: ask again even though an answer is cached (it stays on screen until the new one lands). */
function request(key: string, req: AvatarRequest, refresh = false) {
  if (refresh && inFlight.has(key)) stale.add(key);
  if ((!refresh && answers.has(key)) || inFlight.has(key) || queued.has(key)) return;
  queued.set(key, req);
  schedule();
}

function reaskMounted(match: (key: string) => boolean) {
  for (const key of [...answers.keys()]) {
    if (match(key) && !listeners.has(key)) answers.delete(key);
  }
  for (const key of listeners.keys()) {
    if (!match(key)) continue;
    const [email, auth] = key.split("|");
    request(key, { email, authenticated: auth === "-" ? null : auth === "1" }, true);
  }
}

let wired = false;
function wire() {
  if (wired || typeof window === "undefined") return;
  wired = true;
  void onAvatarsChanged((e) => {
    if (e.all) reaskMounted(() => true);
    else {
      const emails = new Set(e.emails.map((x) => x.trim().toLowerCase()));
      reaskMounted((k) => emails.has(k.split("|")[0]));
    }
  });
  // Turning a source on or off changes every answer.
  let prev: SenderPhotos = currentSettings().senderPhotos;
  let prevOff = currentSettings().avatarPlacement === "off";
  subscribeSettings(() => {
    const next = currentSettings().senderPhotos;
    const off = currentSettings().avatarPlacement === "off";
    if (JSON.stringify(next) !== JSON.stringify(prev) || off !== prevOff) {
      prev = next;
      prevOff = off;
      reaskMounted(() => true);
    }
  });
}

// One stable subscribe function per key, so useSyncExternalStore doesn't
// resubscribe on every render.
const subscribers = new Map<string, (cb: () => void) => () => void>();
function subscribeKey(key: string) {
  let fn = subscribers.get(key);
  if (!fn) subscribers.set(key, (fn = makeSubscribe(key)));
  return fn;
}

function makeSubscribe(key: string) {
  return (cb: () => void) => {
    let set = listeners.get(key);
    if (!set) listeners.set(key, (set = new Set()));
    set.add(cb);
    return () => {
      set.delete(cb);
      if (set.size === 0) listeners.delete(key);
    };
  };
}

/**
 * The photo/logo for a sender, or null while unknown or when there is none
 * (keep the monogram). `authenticated`: the message's senderAuthenticated
 * when known (thread view); leave null in lists.
 */
export function useAvatar(email: string | null | undefined, authenticated: boolean | null = null): AvatarInfo | null {
  const show = useSetting("avatarPlacement") !== "off";
  const key = email ? keyOf(email, authenticated) : "";
  const info = useSyncExternalStore(
    key ? subscribeKey(key) : noopSubscribe,
    () => (key ? answers.get(key) ?? null : null),
  );
  useEffect(() => {
    if (!key || !show || !email) return;
    wire();
    request(key, { email, authenticated });
  }, [key, show, email, authenticated]);
  return show ? info : null;
}

const noopSubscribe = () => () => {};

/** Where avatars go per Settings → "Sender photos appear". */
export function useAvatarPlacement(): { list: boolean; message: boolean } {
  const p: AvatarPlacement = useSetting("avatarPlacement");
  return { list: p === "list" || p === "both", message: p === "message" || p === "both" };
}

/** Settings → "Clear photo cache": forget every answer and loaded URL. */
export function resetAvatars() {
  loadedUrls.clear();
  reaskMounted(() => true);
}
