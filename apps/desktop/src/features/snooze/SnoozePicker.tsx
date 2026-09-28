// H — the snooze picker for the cursor thread or the multi-selection
// (app/selection.ts): Later today · Tomorrow morning · This weekend · Next
// week, or a date and time. ↑↓ move, ↵ or 1–4 pick, Esc closes. When a
// target is already snoozed it shows until when and offers Unsnooze.
import { useEffect, useMemo, useRef, useState } from "react";
import { setUi, useUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { resolveTargets, snooze, snoozedUntil, targets, unsnooze } from "../../app/actions";
import type { ThreadRef } from "../../lib/types";
import { customError, defaultCustom, fmtUntil, fmtWake, fromLocalInput, snoozePresets, toLocalInput } from "./presets";
import { useSnoozeEvents } from "./state";
import "./snooze.css";

export const WAKE_NOTE = "Wakes while Penguin runs, or when it next opens";

export function SnoozePicker() {
  useSnoozeEvents();
  const open = useUi((s) => s.overlay === "snooze");
  if (!open) return null;
  return <Picker />;
}

/** Threads a menu asked for explicitly (a right-clicked row); null = the current targets. */
let explicit: ThreadRef[] | null = null;

/** Open the picker for `refs`, else the selection or cursor thread (H, menus, toolbar). */
export function openSnooze(refs?: ThreadRef[]) {
  explicit = refs ?? null;
  if ((refs ?? targets()).length > 0) setUi({ overlay: "snooze" });
}

function Picker() {
  // Fixed for this opening: what the menu passed, else the selection or cursor thread.
  const [given] = useState(() => explicit);
  const [refs] = useState<ThreadRef[]>(() => given ?? targets());
  const presets = useMemo(() => snoozePresets(), []);
  const current = useMemo(() => {
    const times = refs.map(snoozedUntil).filter((t): t is number => t !== null);
    return times.length ? Math.min(...times) : null;
  }, [refs]);
  const [active, setActive] = useState(0);
  const [custom, setCustom] = useState(() => toLocalInput(current ?? defaultCustom()));
  const [error, setError] = useState<string | null>(null);
  const panel = useRef<HTMLElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const customIndex = presets.length;
  const unsnoozeIndex = current !== null ? presets.length + 1 : -1;
  const count = customIndex + 1 + (current !== null ? 1 : 0);
  useEffect(() => panel.current?.focus(), []);
  useEffect(() => {
    if (active === customIndex) input.current?.focus();
    else if (document.activeElement === input.current) panel.current?.focus();
  }, [active, customIndex]);

  const close = () => setUi({ overlay: null });
  const pick = (at: number) => {
    close();
    if (given) return snooze(given, at);
    // A "select all N matching" selection expands to every thread in the view.
    void resolveTargets().then((all) => snooze(all.length ? all : refs, at));
  };
  const pickCustom = () => {
    const ms = fromLocalInput(custom);
    const why = customError(ms);
    if (why) return setError(why);
    pick(ms);
  };
  const doUnsnooze = () => {
    close();
    unsnooze(refs);
  };
  const run = (i: number) => {
    if (i < presets.length) pick(presets[i].at);
    else if (i === customIndex) pickCustom();
    else if (i === unsnoozeIndex) doUnsnooze();
  };

  const what = refs.length > 1 ? `${num(refs.length)} conversations` : "conversation";
  return (
    <>
      <div className="scrim soft" onMouseDown={close} />
      <div className="overlay-host" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        <section
          ref={panel}
          tabIndex={-1}
          className="palette panel snooze-picker"
          role="dialog"
          aria-label={`Snooze ${what}`}
          onKeyDown={(e) => {
            const inInput = e.target === input.current;
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => (a + (e.key === "ArrowDown" ? 1 : count - 1)) % count);
            } else if (e.key === "Enter") {
              e.preventDefault();
              run(inInput ? customIndex : active);
            } else if (!inInput && /^[1-9]$/.test(e.key) && Number(e.key) <= presets.length) {
              e.preventDefault();
              run(Number(e.key) - 1);
            }
          }}
        >
          <header className="snz-head">
            <Icon name="snooze" size="sm" />
            <span className="grow">Snooze {refs.length > 1 ? what : ""} until…</span>
            {current !== null && <span className="snz-now">Now {fmtUntil(current)}</span>}
          </header>
          <div className="pal-list" role="menu">
            {presets.map((p, i) => (
              <button
                key={p.id}
                role="menuitem"
                className={"menu-item" + (i === active ? " active" : "")}
                onMouseMove={() => setActive(i)}
                onClick={() => pick(p.at)}
              >
                <Icon name="clock" size="sm" />
                <span className="grow">{p.label}</span>
                <span className="snz-when">{fmtWake(p.at)}</span>
                <Kbd>{String(i + 1)}</Kbd>
              </button>
            ))}
            <div
              className={"menu-item snz-custom" + (active === customIndex ? " active" : "")}
              onMouseMove={() => setActive(customIndex)}
            >
              <Icon name="calendar" size="sm" />
              <input
                ref={input}
                type="datetime-local"
                className="input snz-input"
                value={custom}
                min={toLocalInput(Date.now())}
                onFocus={() => setActive(customIndex)}
                onChange={(e) => {
                  setCustom(e.target.value);
                  setError(null);
                }}
                aria-label="Snooze until a date and time"
              />
              <button className="btn btn-secondary btn-sm" onClick={pickCustom}>
                Snooze
              </button>
            </div>
            {error && <div className="snz-error">{error}</div>}
            {current !== null && (
              <button
                role="menuitem"
                className={"menu-item" + (active === unsnoozeIndex ? " active" : "")}
                onMouseMove={() => setActive(unsnoozeIndex)}
                onClick={doUnsnooze}
              >
                <Icon name="inbox" size="sm" />
                <span className="grow">Unsnooze now</span>
                <span className="snz-when">Back to the inbox</span>
              </button>
            )}
          </div>
          <footer className="pal-foot">
            <span className="grow faint">{WAKE_NOTE}</span>
            <span className="hint">
              <Kbd>↵</Kbd>Snooze
            </span>
            <span className="hint">
              <Kbd>Esc</Kbd>Close
            </span>
          </footer>
        </section>
      </div>
    </>
  );
}
