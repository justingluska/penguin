// Screens most sessions never open (Settings, onboarding, Add account, the
// calendar) live in their own chunks (Vite code splitting), so launch parses
// and runs only what the mail view needs. Each is fetched in the background
// once the first rows are on screen (prefetchScreens), so opening one later
// still renders in the same frame as the key press. Before that, the first
// open renders nothing for the frame or two the chunk takes to load.
//
// Not React.lazy: a lazy component suspends on its first render even when its
// chunk has already loaded (the status is only read back from a promise), and
// that would cost a frame on every first open.
import { createElement, useEffect, useState, type ComponentType } from "react";

type Preload = () => Promise<unknown>;
const queue: Preload[] = [];

export interface LazyScreen<P> {
  (props: P): ReturnType<typeof createElement> | null;
  /** Fetch and evaluate the chunk now (idempotent). */
  preload: () => Promise<ComponentType<P>>;
}

/**
 * A component whose code loads on first render (or on preload), then renders
 * synchronously ever after. `prefetch: false` leaves it out of prefetchScreens
 * (screens a set-up app never shows, such as first-run setup).
 */
export function lazyScreen<P extends object>(load: () => Promise<ComponentType<P>>, { prefetch = true } = {}): LazyScreen<P> {
  let loaded: ComponentType<P> | null = null;
  let pending: Promise<ComponentType<P>> | null = null;
  const preload = () =>
    (pending ??= load().then(
      (c) => (loaded = c),
      (e: unknown) => {
        // Let the next render try again; the rejection still reaches the
        // caller (and main.tsx's unhandledrejection log).
        pending = null;
        throw e;
      },
    ));
  function Lazy(props: P) {
    const [, setReady] = useState(false);
    useEffect(() => {
      if (!loaded) void preload().then(() => setReady(true));
    }, []);
    return loaded ? createElement(loaded, props) : null;
  }
  if (prefetch) queue.push(preload);
  return Object.assign(Lazy, { preload });
}

type IdleWindow = Window & { requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number };

/** Run `f` when the main thread is idle (WebKit has no requestIdleCallback: a short timeout there). */
export function whenIdle(f: () => void, timeout = 1000): void {
  const w = window as IdleWindow;
  if (w.requestIdleCallback) w.requestIdleCallback(f, { timeout });
  else setTimeout(f, 50);
}

let started = false;
/**
 * Fetch every lazy screen (and any extra loaders, such as the compose
 * editor) one at a time in idle moments, so no single task gets long. Called
 * once the first mail rows have painted; later calls do nothing.
 */
export function prefetchScreens(extra: Preload[] = []): void {
  if (started) return;
  started = true;
  const todo = [...extra, ...queue];
  const next = () => {
    const p = todo.shift();
    if (!p) return;
    void p().then(() => whenIdle(next));
  };
  whenIdle(next);
}
