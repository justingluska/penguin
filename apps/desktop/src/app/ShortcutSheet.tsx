// "?" cheat sheet: every key the app answers to, grouped, in the ⌘K
// palette's visual language (design/05-command.html). What's listed, and why
// it's more than the registry: app/keyCatalog.ts.
import { Fragment, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { getShortcuts, isMac, subscribeShortcuts } from "../lib/keyboard";
import { setUi, useUi } from "../lib/ui";
import { Icon } from "../components/Icon";
import { Keys, Kbd } from "../components/Kbd";
import { sheetGroups } from "./keyCatalog";

export function ShortcutSheet() {
  const open = useUi((s) => s.overlay === "shortcuts");
  if (!open) return null;
  return <Sheet />;
}

function Sheet() {
  const version = useSyncExternalStore(subscribeShortcuts, () => getShortcuts().length);
  const [q, setQ] = useState("");
  const groups = useMemo(() => sheetGroups(getShortcuts(), { mac: isMac, q }), [q, version]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setUi({ overlay: null });
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  return (
    <>
      <div className="scrim soft" onMouseDown={() => setUi({ overlay: null })} />
      <div className="overlay-host" onMouseDown={(e) => e.target === e.currentTarget && setUi({ overlay: null })}>
        <section className="palette panel sheet keys-always" role="dialog" aria-label="Keyboard shortcuts">
          <div className="pal-input">
            <Icon name="keyboard" size="sm" className="faint" />
            <input
              className="pal-field"
              autoFocus
              placeholder="Filter shortcuts…"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "?" && !q) {
                  e.preventDefault();
                  setUi({ overlay: null });
                }
              }}
            />
          </div>
          <div className="pal-list sheet-list">
            {groups.map((g) => (
              <div key={g.name} className="sheet-group">
                <div className="pal-group">{g.name}</div>
                {g.rows.map((r) => (
                  <div key={r.id} className="menu-item">
                    <span className="grow">{r.label}</span>
                    <span className="sheet-keys">
                      {r.keys.map((k, i) => (
                        <Fragment key={k}>
                          {i > 0 && <span className="kbd-then">or</span>}
                          <Keys keys={k} />
                        </Fragment>
                      ))}
                    </span>
                  </div>
                ))}
              </div>
            ))}
            {groups.length === 0 && <div className="pal-group">No shortcuts match “{q}”</div>}
          </div>
          <footer className="pal-foot">
            <span className="hint faint">Every action, with its shortcut</span>
            <span className="grow" />
            <span className="hint">
              <Kbd>⌘K</Kbd>Commands
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
