// Spark-style swipe actions on list rows, DOM side. One PASSIVE wheel listener
// on the list (a non-passive one makes the engine wait for JS before every
// scroll step, which is what made scrolling lag); the gesture state machine
// is in swipe.ts. Scrolling costs a few property reads per event: no layout
// reads, no timers, no React state. Only a locked horizontal swipe does real
// work, and then only on the one row (its transform and a few attributes).
// The list can't scroll sideways (overflow-x: hidden), so nothing needs to be
// prevented for a horizontal swipe.
//
// The action panel behind the row is created on the fly (static, trusted
// markup) inside the row's wrapper (.v-inner) and removed when the gesture
// ends.
//
// Feel: the row tracks the fingers 1:1 and rubber-bands past its limits
// (swipe.ts); the action's icon grows in with the distance, then snaps into
// its tone with a small bump when the action arms. On release the action runs
// at once: a removing action flies the row off in the swipe's direction and
// the list collapses the gap (listMotion.ts); anything else springs back.
import { useEffect, type RefObject } from "react";
import type { SwipeAction, ThreadRef } from "../../lib/types";
import { currentSettings } from "../../lib/settings";
import { list } from "../../app/store";
import { archive, toggleRead, toggleStar, trash } from "../../app/actions";
import { hasSelection } from "../../app/selection";
import { SWIPE, feedSwipe, initialSwipe, releaseSwipe, rubberSwipe, swipeProgress, zoneOf, type SwipeAvail, type SwipeState, type SwipeZone } from "./swipe";
import { hintExit } from "./rowExit";

interface ActionMeta {
  icon: string;
  tone: string;
  label: (t: { unread: boolean; starred: boolean }) => string;
  /** Removes the row from the list (animates out) vs. changes it in place (springs back). */
  removes: boolean;
}

const META: Record<Exclude<SwipeAction, "none">, ActionMeta> = {
  toggleRead: { icon: "unread", tone: "blue", label: (t) => (t.unread ? "Mark read" : "Mark unread"), removes: false },
  star: { icon: "star", tone: "amber", label: (t) => (t.starred ? "Unstar" : "Star"), removes: false },
  archive: { icon: "archive", tone: "green", label: () => "Archive", removes: true },
  trash: { icon: "trash", tone: "red", label: () => "Trash", removes: true },
};

type Zones = Record<Exclude<SwipeZone, null>, SwipeAction>;

function zonesFromSettings(): Zones {
  const s = currentSettings();
  return { right: s.swipeRight, left: s.swipeLeft, leftLong: s.swipeLeftLong };
}

const NO_ZONES: Zones = { right: "none", left: "none", leftLong: "none" };

const availOf = (z: Zones): SwipeAvail => ({ right: z.right !== "none", left: z.left !== "none", leftLong: z.leftLong !== "none" });

function run(action: SwipeAction, ref: ThreadRef) {
  switch (action) {
    case "toggleRead": return toggleRead([ref]);
    case "star": return toggleStar([ref]);
    case "archive": return void archive([ref]);
    case "trash": return void trash([ref]);
  }
}

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Wheel deltas in px (line/page modes are rare on macOS but possible with mice). */
function px(e: WheelEvent): [number, number] {
  const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1;
  return [e.deltaX * k, e.deltaY * k];
}

interface Active {
  ref: ThreadRef;
  key: string;
  wrap: HTMLElement;
  row: HTMLElement;
  bg: HTMLElement | null;
  width: number;
  zones: Zones;
  zone: SwipeZone;
  side: "right" | "left" | null;
  motion: boolean;
}

/** Threads with a committed swipe whose action hasn't settled: no double commits. */
const committing = new Set<string>();

/** `mounted`: whether the scroll element is rendered (the list swaps in empty states). */
export function useRowSwipe(scrollRef: RefObject<HTMLElement | null>, mounted: boolean) {
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    let state: SwipeState = initialSwipe();
    let active: Active | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const begin = (target: EventTarget | null): Active | null => {
      const row = (target as Element | null)?.closest?.<HTMLElement>(".msg[data-thread]");
      const wrap = row?.parentElement;
      if (!row || !wrap) return null;
      const ref = { accountId: row.dataset.account!, threadId: row.dataset.thread! };
      const key = ref.accountId + "\u0000" + ref.threadId;
      if (committing.has(key)) return null;
      // While conversations are selected, gestures would be ambiguous: no swipes.
      if (hasSelection()) return null;
      // Width, settings and motion preference are read once the gesture locks
      // horizontal (see onWheel): a vertical scroll never pays for them.
      return { ref, key, wrap, row, bg: null, width: 0, zones: NO_ZONES, zone: null, side: null, motion: true };
    };

    const paint = (a: Active, offset: number) => {
      if (!a.motion) return;
      const side = offset > 0 ? "right" : offset < 0 ? "left" : null;
      const zone = zoneOf(offset, a.width, availOf(a.zones));
      if (side !== a.side) {
        a.side = side;
        a.bg?.remove();
        a.bg = side ? panel(a, side) : null;
        if (a.bg) a.wrap.prepend(a.bg);
      }
      if (zone !== a.zone && a.bg) {
        a.zone = zone;
        fillPanel(a.bg, a, side!, zone);
      }
      // The icon grows in with the distance until the action arms (styles/app.css).
      a.bg?.style.setProperty("--p", swipeProgress(offset, a.width).toFixed(3));
      a.row.style.transition = "none";
      a.row.style.transform = offset ? `translateX(${offset}px)` : "";
      a.wrap.classList.toggle("is-swiping", offset !== 0);
    };

    const settle = (a: Active, zone: SwipeZone) => {
      const action = zone ? a.zones[zone] : "none";
      const meta = action !== "none" ? META[action] : null;
      const cleanup = () => {
        a.row.style.transition = "";
        a.row.style.transform = "";
        a.bg?.remove();
        a.wrap.classList.remove("is-swiping");
      };
      if (!meta) {
        springBack(a, cleanup);
        return;
      }
      committing.add(a.key);
      const done = () => committing.delete(a.key);
      // A flick commits short of the threshold: show the action armed as it goes.
      if (a.bg && a.side && zone !== a.zone) {
        a.zone = zone;
        fillPanel(a.bg, a, a.side, zone);
        a.bg.style.setProperty("--p", "1");
      }
      if (meta.removes && a.motion) {
        // Fly the rest of the way out and run the action now (never after the
        // animation). The list keeps the row as a ghost while it flies and
        // collapses its gap; the hint (set after the action's own) says "don't
        // slide it again".
        const dir = zone === "right" ? 1 : -1;
        a.row.style.transition = `transform ${FLY_MS}ms cubic-bezier(.25, .6, .35, 1)`;
        a.row.style.transform = `translateX(${dir * (a.width + 16)}px)`;
        run(action, a.ref);
        hintExit([a.ref], "swipe");
        if (inList(a.ref)) {
          // It stays (e.g. archive outside the inbox): come back in place.
          setTimeout(() => springBack(a, () => (cleanup(), done())), FLY_MS);
        } else {
          // Gone: its node leaves with the ghost; just free the key.
          setTimeout(done, FLY_MS + 120);
        }
      } else {
        run(action, a.ref);
        if (a.motion) springBack(a, () => (cleanup(), done()));
        else done();
      }
    };

    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey) return; // pinch-zoom
      const [dx, dy] = px(e);
      const now = e.timeStamp || performance.now();
      if (state.phase === "cooldown") {
        if (now < state.until) {
          // Momentum tail of a finished swipe: swallow, keep the cooldown alive.
          state = feedSwipe(state, dx, dy, now, 0);
          rearm(SWIPE.IDLE_MS);
          return;
        }
        state = initialSwipe();
      }
      if (state.phase !== "idle" && now - state.last > SWIPE.IDLE_MS) finish(); // missed end
      if (state.phase === "idle") {
        active = begin(e.target);
        if (!active) return; // not over a row: plain scrolling
      }
      if (!active) return;
      state = feedSwipe(state, dx, dy, now, active.width || Infinity, active.width ? availOf(active.zones) : undefined);
      if (state.phase === "tracking") {
        if (!active.width) {
          active.width = active.row.offsetWidth;
          active.zones = zonesFromSettings();
          active.motion = !reducedMotion();
          state = { ...state, offset: rubberSwipe(state.raw, active.width, availOf(active.zones)) };
        }
        paint(active, state.offset);
        rearm(SWIPE.IDLE_MS);
      } else if (state.phase === "deciding") {
        rearm(SWIPE.IDLE_MS);
      }
      // "ignoring" (a vertical scroll): no timer; the next gesture's first
      // event sees the gap and starts over.
    };

    const rearm = (ms: number) => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(finish, ms);
    };

    /** The gesture went quiet: commit or spring back, then cool down. */
    const finish = () => {
      if (timer) clearTimeout(timer);
      timer = null;
      if (state.phase === "cooldown" || state.phase === "idle") {
        state = initialSwipe();
        return;
      }
      const a = active;
      active = null;
      const r = a ? releaseSwipe(state, performance.now(), a.width, availOf(a.zones)) : { zone: null, next: initialSwipe() };
      state = r.next;
      if (a && state.phase === "cooldown") {
        settle(a, r.zone);
        rearm(SWIPE.COOLDOWN_MS);
      }
    };

    el.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      el.removeEventListener("wheel", onWheel);
      if (timer) clearTimeout(timer);
    };
  }, [scrollRef, mounted]);
}

/** How long a committed row takes to fly off. */
const FLY_MS = 180;
/** The spring back (--dur-spring / --ease-spring in penguin.css). */
const SPRING_MS = 320;

function inList(ref: ThreadRef): boolean {
  return list.get().items.some((t) => t.threadId === ref.threadId && t.accountId === ref.accountId);
}

function springBack(a: Active, then: () => void) {
  if (!a.motion) return then();
  a.row.style.transition = `transform ${SPRING_MS}ms var(--ease-spring)`;
  a.row.style.transform = "";
  a.bg?.classList.add("is-leaving");
  setTimeout(then, SPRING_MS);
}

function panel(a: Active, side: "right" | "left"): HTMLElement {
  const bg = document.createElement("div");
  bg.className = `swipe-bg is-${side}`;
  bg.setAttribute("aria-hidden", "true");
  fillPanel(bg, a, side, null);
  return bg;
}

/** Show the action for the current zone; below the threshold, preview the side's first action muted. */
function fillPanel(bg: HTMLElement, a: Active, side: "right" | "left", zone: SwipeZone) {
  const action = zone ? a.zones[zone] : side === "right" ? a.zones.right : a.zones.left !== "none" ? a.zones.left : a.zones.leftLong;
  const meta = action !== "none" ? META[action] : null;
  const t = list.get().items.find((x) => x.threadId === a.ref.threadId && x.accountId === a.ref.accountId);
  bg.className = `swipe-bg is-${side}` + (zone ? ` is-armed t-${meta?.tone ?? "gray"}` : "");
  // Re-trigger the snap animation on every zone change.
  bg.dataset.zone = zone ?? "";
  if (!meta) {
    bg.innerHTML = "";
    return;
  }
  bg.innerHTML =
    `<span class="swipe-act"><svg class="i" aria-hidden="true"><use href="#i-${meta.icon}"/></svg>` +
    `<span>${meta.label({ unread: t?.unread ?? false, starred: t?.starred ?? false })}</span></span>`;
}

