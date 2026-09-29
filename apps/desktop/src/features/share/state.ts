// Share links, runtime state: whether storage is set up (for the menus'
// "Copy Share Link" vs "Copy Share Link…"), and the file waiting for setup
// to finish. The waiting file lives in this window's memory only: closing
// Settings or Cancel drops it, and nothing uploads.
import { useSyncExternalStore } from "react";
import { api } from "../../lib/api";
import type { ShareLinkConfig } from "../../lib/types";
import type { ShareTarget } from "./model";
import { isSettingsOpen, subscribeSettingsOpen } from "../settings/state";

let config: ShareLinkConfig | null = null;
let loading: Promise<void> | null = null;
let pending: ShareTarget | null = null;
const subs = new Set<() => void>();
const notify = () => subs.forEach((f) => f());
const subscribe = (cb: () => void) => {
  subs.add(cb);
  return () => subs.delete(cb);
};

/** Set up, as last read (false until the first read lands). */
export function shareReady(): boolean {
  return !!config?.configured;
}

/** Read the settings again (cheap: a small file, never the Keychain). */
export function refreshShareStatus(): Promise<void> {
  loading ??= api
    .shareLinkConfigGet()
    .then(setShareConfig, () => undefined)
    .finally(() => {
      loading = null;
    });
  return loading;
}

export function setShareConfig(c: ShareLinkConfig) {
  config = c;
  notify();
}

export function useShareConfig(): ShareLinkConfig | null {
  return useSyncExternalStore(subscribe, () => config);
}

/** Remember the file to share once setup and its test succeed. */
export function setPendingShare(t: ShareTarget | null) {
  pending = t;
  notify();
}

// Closing Settings drops the waiting file: nothing uploads unless the user
// finishes setup while it is open.
subscribeSettingsOpen(() => {
  if (pending && !isSettingsOpen()) setPendingShare(null);
});

export function takePendingShare(): ShareTarget | null {
  const t = pending;
  pending = null;
  notify();
  return t;
}

export function usePendingShare(): ShareTarget | null {
  return useSyncExternalStore(subscribe, () => pending);
}
