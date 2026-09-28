// The app's one notification system: a deck of toasts, bottom right, above
// the status bar. OWNER: ui-qol (took over from ui-inbox).
//
// Layout (geometry in toastStack.ts): collapsed, the newest toast is in front
// and up to two older ones peek out behind it; hovering or focusing the deck
// fans it out into a list, and every timer pauses while it's open. Cards move
// with transform/opacity/clip-path only. New cards slide in at the front;
// dismissed ones fade out while the rest move up. Drag a card sideways to
// dismiss it.
//
// Kinds (each has its own icon and default lifetime):
//   info      a plain outcome ("Marked as read")                    4s
//   success   something finished well ("Everything's up to date")  3s
//   action    an outcome with a follow-up ("Archived · Undo Z")      6s
//   progress  something running ("Sending in 5s", "Checking…")    until updated/dismissed
//   error     a failure; stays until dismissed, or resolved by its owner
//
// - At most STACK.VISIBLE show; more wait behind the front card, counted in a
//   "N more" badge. Past MAX_KEPT the oldest transient one leaves (errors last).
// - The same message (or the same `key`) never stacks: it merges into the
//   existing toast, bumps a ×N count and restarts its timer. Owners that
//   update one toast over time (a countdown, a sync issue) use a `key`.
// - Action buttons sit on their own row under the message (left-aligned with
//   it); toasts without actions stay one compact row.
// - Toasts never take focus. Errors are role="alert", the rest role="status".
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties } from "react";
import { Icon, type IconName } from "./Icon";
import { Keys } from "./Kbd";
import { STACK, stackHeight, stackLayout, type CardPlacement } from "./toastStack";
import { logClientEvent } from "../lib/api";

export type ToastKind = "info" | "success" | "action" | "progress" | "error";

export interface ToastAction {
  label: string;
  /** Shortcut shown on the button (the key itself is registered elsewhere). */
  keys?: string;
  run: () => unknown;
  /** Keep the toast after running (e.g. Retry while the retry is in flight). */
  keep?: boolean;
}

export interface ToastInput {
  message: string;
  kind?: ToastKind;
  /** Legacy: tone "error" is kind "error". */
  tone?: "default" | "error";
  /** Second line, quieter (e.g. which accounts). */
  detail?: string;
  /** Tooltip on the message (raw error text). */
  title?: string;
  action?: ToastAction;
  actions?: ToastAction[];
  /** ms before it leaves; null = stays until dismissed. Defaults by kind. */
  duration?: number | null;
  /** Merge key: a toast with the same key is updated instead of added. */
  key?: string;
  /** progress: 0..1 draws a bar; null/undefined shows a spinner. */
  progress?: number | null;
}

export interface ToastSpec extends Omit<ToastInput, "tone" | "action"> {
  id: number;
  kind: ToastKind;
  key: string;
  actions: ToastAction[];
  /** How many times this message arrived (merged duplicates). */
  count: number;
  /** Dismissed, fading out; no longer part of the deck. */
  leaving?: boolean;
}

const MAX_KEPT = 10;
/** Exit animation length (the card is removed after it). */
const LEAVE_MS = 200;
const DEFAULT_MS: Record<ToastKind, number | null> = { info: 4000, success: 3000, action: 6000, progress: null, error: null };

let toasts: ToastSpec[] = [];
let seq = 0;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

// Timers with pause/resume: remaining ms per toast while paused.
const timers = new Map<number, { handle: ReturnType<typeof setTimeout> | null; remaining: number; startedAt: number }>();
let paused = false;

function arm(id: number, ms: number | null) {
  clear(id);
  if (ms === null) return;
  const t = { handle: null as ReturnType<typeof setTimeout> | null, remaining: ms, startedAt: Date.now() };
  if (!paused) t.handle = setTimeout(() => dismissToast(id), ms);
  timers.set(id, t);
}
function clear(id: number) {
  const t = timers.get(id);
  if (t?.handle) clearTimeout(t.handle);
  timers.delete(id);
}

function normalize(t: ToastInput): Omit<ToastSpec, "id" | "count"> {
  const actions = t.actions ?? (t.action ? [t.action] : []);
  const kind: ToastKind = t.kind ?? (t.tone === "error" ? "error" : actions.length ? "action" : "info");
  return {
    message: t.message,
    kind,
    detail: t.detail,
    title: t.title,
    actions,
    duration: t.duration,
    key: t.key ?? `${kind}:${t.message}`,
    progress: t.progress,
  };
}

const lifetime = (t: { kind: ToastKind; duration?: number | null }) => (t.duration !== undefined ? t.duration : DEFAULT_MS[t.kind]);

/** Show a toast (or merge into the one with the same key). Returns its id. */
export function toast(input: ToastInput): number {
  const n = normalize(input);
  // What the user was told went wrong is in penguin.log too (View log).
  if (n.kind === "error") {
    logClientEvent({ level: "error", source: "toast", what: "toast", message: [n.title, n.message, n.detail].filter(Boolean).join(" · ") });
  }
  const existing = toasts.find((t) => t.key === n.key && !t.leaving);
  if (existing) {
    // Same message again: one toast with a count, newest position, fresh timer.
    // An explicit key is an owner updating its toast: replace, don't count.
    const merged: ToastSpec = { ...existing, ...n, id: existing.id, count: input.key ? existing.count : existing.count + 1 };
    toasts = [...toasts.filter((t) => t.id !== existing.id), merged];
    arm(existing.id, lifetime(merged));
    emit();
    return existing.id;
  }
  const id = ++seq;
  toasts = [...toasts, { ...n, id, count: 1 }];
  const live = () => toasts.filter((t) => !t.leaving);
  while (live().length > MAX_KEPT) {
    // Oldest transient toast leaves first; errors only when nothing else can.
    const victim = live().find((t) => t.kind !== "error" && t.id !== id) ?? live()[0];
    clear(victim.id);
    toasts = toasts.filter((t) => t.id !== victim.id);
  }
  arm(id, lifetime(n));
  emit();
  return id;
}

/**
 * Update a toast in place (a countdown, a progress step, a result). Keeps its
 * timer unless `duration` or `kind` changes. Pass `key` changes at your peril.
 */
export function updateToast(id: number, patch: Partial<ToastInput>) {
  const cur = toasts.find((t) => t.id === id);
  if (!cur || cur.leaving) return;
  const actions = patch.actions ?? (patch.action ? [patch.action] : cur.actions);
  const kind = patch.kind ?? (patch.tone === "error" ? "error" : cur.kind);
  const next: ToastSpec = {
    ...cur,
    ...(patch.message !== undefined && { message: patch.message }),
    ...("detail" in patch && { detail: patch.detail }),
    ...("title" in patch && { title: patch.title }),
    ...("progress" in patch && { progress: patch.progress }),
    ...("duration" in patch && { duration: patch.duration }),
    kind,
    actions,
  };
  toasts = toasts.map((t) => (t.id === id ? next : t));
  if ("duration" in patch || kind !== cur.kind) arm(id, lifetime(next));
  emit();
}

export function dismissToast(id: number) {
  clear(id);
  const t = toasts.find((x) => x.id === id);
  if (!t || t.leaving) return;
  // Fade out first; the deck re-forms around it right away.
  toasts = toasts.map((x) => (x.id === id ? { ...x, leaving: true } : x));
  emit();
  setTimeout(() => {
    toasts = toasts.filter((x) => x.id !== id);
    emit();
  }, LEAVE_MS);
}

/** Dismiss the toast with this key, if showing (e.g. a sync issue got resolved). */
export function dismissToastKey(key: string) {
  const t = toasts.find((x) => x.key === key && !x.leaving);
  if (t) dismissToast(t.id);
}

/** Id of the toast showing under `key`, if any. */
export function toastIdForKey(key: string): number | null {
  return toasts.find((t) => t.key === key && !t.leaving)?.id ?? null;
}

function setPaused(p: boolean) {
  if (p === paused) return;
  paused = p;
  const now = Date.now();
  for (const [id, t] of timers) {
    if (p) {
      if (t.handle) clearTimeout(t.handle);
      t.handle = null;
      t.remaining = Math.max(0, t.remaining - (now - t.startedAt));
    } else {
      t.startedAt = now;
      // A little grace after the pointer leaves, so it doesn't vanish instantly.
      t.handle = setTimeout(() => dismissToast(id), Math.max(t.remaining, 1200));
    }
  }
}

const ICON: Partial<Record<ToastKind, IconName>> = { success: "check", error: "info" };

/** Hover-out grace before the deck folds back (so crossing a gap doesn't flicker). */
const COLLAPSE_DELAY_MS = 150;
/** Drag distance that dismisses a card. */
const SWIPE_DISMISS_PX = 80;

export function ToastHost() {
  const list = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => toasts,
  );
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [heights, setHeights] = useState<Record<number, number>>({});
  const collapseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Last placement per card, so a leaving card fades out where it stood. */
  const lastPlace = useRef(new Map<number, CardPlacement>());

  // Heights come from ResizeObserver (async, no forced layout on our side).
  const ro = useMemo(
    () =>
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver((entries) =>
            setHeights((prev) => {
              let next = prev;
              for (const e of entries) {
                const id = Number((e.target as HTMLElement).dataset.toastId);
                const h = Math.round(e.borderBoxSize?.[0]?.blockSize ?? (e.target as HTMLElement).offsetHeight);
                if (prev[id] !== h) {
                  if (next === prev) next = { ...prev };
                  next[id] = h;
                }
              }
              return next;
            }),
          ),
    [],
  );
  useEffect(() => () => ro?.disconnect(), [ro]);

  const live = list.filter((t) => !t.leaving).reverse(); // newest first
  const expanded = (hovered || focused) && live.length > 1;
  // Timers pause while the deck is open (hover or keyboard focus).
  useEffect(() => {
    setPaused(hovered || focused);
  }, [hovered, focused]);
  useEffect(() => {
    if (list.length === 0) {
      setHovered(false);
      setFocused(false);
    }
  }, [list.length]);

  if (list.length === 0) return null;

  const liveHeights = live.map((t) => heights[t.id]);
  const place = stackLayout(liveHeights, expanded);
  const byId = new Map(live.map((t, i) => [t.id, { i, p: place[i] }]));
  for (const [id, { p }] of byId) lastPlace.current.set(id, p);
  for (const id of lastPlace.current.keys()) if (!list.some((t) => t.id === id)) lastPlace.current.delete(id);

  // "N more": cards the deck doesn't fully show. Collapsed, the ones behind the
  // front count once any of them is an error or some are past the cap.
  const behind = live.slice(1);
  const moreCount = expanded
    ? Math.max(0, live.length - STACK.VISIBLE)
    : behind.length > 0 && (behind.some((t) => t.kind === "error") || live.length > STACK.VISIBLE)
      ? behind.length
      : 0;

  const enter = () => {
    if (collapseTimer.current) clearTimeout(collapseTimer.current);
    collapseTimer.current = null;
    setHovered(true);
  };
  const leave = () => {
    if (collapseTimer.current) clearTimeout(collapseTimer.current);
    collapseTimer.current = setTimeout(() => setHovered(false), COLLAPSE_DELAY_MS);
  };

  return (
    <section
      className={"toast-host" + (expanded ? " is-expanded" : "")}
      // The host covers exactly the deck, so hovering the gaps between
      // expanded cards doesn't count as leaving.
      style={{ height: stackHeight(liveHeights, expanded) }}
      aria-label="Notifications"
      onMouseEnter={enter}
      onMouseLeave={leave}
      onFocus={() => setFocused(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setFocused(false);
      }}
    >
      {list.map((t) => {
        const cur = byId.get(t.id);
        const p = cur?.p ?? lastPlace.current.get(t.id) ?? null;
        return (
          <ToastView
            key={t.id}
            t={t}
            place={p}
            front={cur?.i === 0}
            more={cur?.i === 0 ? moreCount : 0}
            ro={ro}
          />
        );
      })}
    </section>
  );
}

function ToastView({
  t,
  place,
  front,
  more,
  ro,
}: {
  t: ToastSpec;
  place: CardPlacement | null;
  front: boolean;
  more: number;
  ro: ResizeObserver | null;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [mounted, setMounted] = useState(false);
  const drag = useRef<{ x: number; id: number; dx: number } | null>(null);
  const icon = ICON[t.kind];
  const determinate = t.kind === "progress" && typeof t.progress === "number";
  const hasActs = t.actions.length > 0;

  useEffect(() => {
    const el = ref.current;
    if (!el || !ro) return;
    ro.observe(el);
    return () => ro.unobserve(el);
  }, [ro]);
  // Two frames so the off-screen start state is painted before sliding in.
  useEffect(() => {
    let r2 = 0;
    const r1 = requestAnimationFrame(() => (r2 = requestAnimationFrame(() => setMounted(true))));
    return () => {
      cancelAnimationFrame(r1);
      cancelAnimationFrame(r2);
    };
  }, []);

  const p = place ?? { y: 0, scale: 1, opacity: 1, dim: 0, clipBottom: 0, z: 1000, hidden: false };
  const style = {
    "--ty": `${mounted ? p.y : p.y + 24}px`,
    "--s": p.scale,
    "--clip": `${p.clipBottom}px`,
    "--dim": p.dim,
    opacity: !mounted || t.leaving ? 0 : p.opacity,
    zIndex: p.z,
  } as CSSProperties;

  return (
    <div
      ref={ref}
      data-toast-id={t.id}
      className={
        `toast panel toast-${t.kind}` +
        (hasActs ? " has-acts" : "") +
        (t.kind === "error" ? " t-red" : "") +
        (front ? " is-front" : " is-behind") +
        (p.hidden ? " is-hidden" : "") +
        (t.leaving ? " is-leaving" : "")
      }
      style={style}
      role={t.kind === "error" ? "alert" : "status"}
      aria-live={t.kind === "error" ? "assertive" : "polite"}
      aria-hidden={p.hidden || t.leaving ? true : undefined}
      onPointerDown={(e) => {
        if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
        drag.current = { x: e.clientX, id: e.pointerId, dx: 0 };
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        d.dx = e.clientX - d.x;
        e.currentTarget.classList.add("is-dragging");
        e.currentTarget.style.setProperty("--tx", `${d.dx}px`);
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        if (!d) return;
        e.currentTarget.classList.remove("is-dragging");
        if (Math.abs(d.dx) >= SWIPE_DISMISS_PX) {
          e.currentTarget.style.setProperty("--tx", `${Math.sign(d.dx) * 420}px`);
          dismissToast(t.id);
        } else {
          e.currentTarget.style.setProperty("--tx", "0px");
        }
      }}
      onPointerCancel={(e) => {
        drag.current = null;
        e.currentTarget.classList.remove("is-dragging");
        e.currentTarget.style.setProperty("--tx", "0px");
      }}
    >
      {t.kind === "progress" && !determinate ? (
        <span className="toast-spin" aria-hidden="true" />
      ) : icon ? (
        <Icon name={icon} size="xs" className="toast-ico" />
      ) : null}
      {/* Text, then the actions on their own row under it (left-aligned with the text). */}
      <div className="toast-body">
        <span className="toast-text" title={t.title}>
          <span className="toast-msg">
            {t.message}
            {t.count > 1 && <span className="toast-count tnum"> ×{t.count}</span>}
          </span>
          {t.detail && <span className="toast-detail">{t.detail}</span>}
        </span>
        {hasActs && (
          <div className="toast-acts">
            {t.actions.map((a) => (
              <button
                key={a.label}
                className="btn btn-ghost btn-sm toast-act"
                onMouseDown={(e) => e.preventDefault() /* never take focus from the list or composer */}
                onClick={() => {
                  const r = a.run();
                  if (!a.keep) dismissToast(t.id);
                  return r;
                }}
              >
                {a.label}
                {a.keys && <Keys keys={a.keys} />}
              </button>
            ))}
          </div>
        )}
      </div>
      {more > 0 && <span className="toast-more tnum">{more} more</span>}
      <button
        className="btn btn-ghost btn-sm btn-icon toast-x"
        aria-label="Dismiss"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => dismissToast(t.id)}
      >
        <Icon name="x" size="xs" />
      </button>
      {determinate && (
        <span className="toast-bar" aria-hidden="true">
          <span style={{ width: `${Math.round(Math.max(0, Math.min(1, t.progress!)) * 100)}%` }} />
        </span>
      )}
    </div>
  );
}
