// Account and profile color pickers: the 40-color palette (lib/themes.ts) as
// a grid, ten hue columns by four rows. OWNER: themes.
//
// <ColorGrid> is the Settings popover's body: a radio group with one tab
// stop, arrows move (components/menuModel.ts gridStep), Enter or Space picks,
// Esc is left to the popover around it. colorSubmenu() is the same grid as a
// context-menu submenu (the sidebar's right-click "Color"), where the menu
// host drives the keys. Swatches show the color as picked, with a check on
// the current one and the color's name as the tooltip.
import { useRef, useState, type KeyboardEvent } from "react";
import { ACCOUNT_COLORS, ACCOUNT_COLOR_COLUMNS } from "../lib/themes";
import { accountTone } from "../lib/accountColor";
import { gridStep, type MenuItem } from "./menuModel";
import { Icon } from "./Icon";

const same = (a: string | null | undefined, b: string) => (a ?? "").toLowerCase() === b.toLowerCase();

/** One round swatch; `selected` adds the ring and the check. */
export function ColorSwatch({ hex, selected }: { hex: string; selected: boolean }) {
  return (
    <span className={`acc-swatch t-${accountTone(hex)}` + (selected ? " selected" : "")} aria-hidden="true">
      {selected && <Icon name="check" size="xs" />}
    </span>
  );
}

export function ColorGrid({ value, label, onPick }: { value: string | null | undefined; label: string; onPick: (hex: string) => void }) {
  const current = ACCOUNT_COLORS.findIndex((c) => same(value, c.hex));
  const [focus, setFocus] = useState(Math.max(0, current));
  const ref = useRef<HTMLDivElement>(null);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      e.stopPropagation();
      onPick(ACCOUNT_COLORS[focus].hex);
      return;
    }
    const to = gridStep(focus, e.key, ACCOUNT_COLORS.length, ACCOUNT_COLOR_COLUMNS);
    if (to === null) return;
    e.preventDefault();
    e.stopPropagation();
    if (typeof to === "number" && to >= 0) {
      setFocus(to);
      ref.current?.querySelector<HTMLButtonElement>(`[data-index="${to}"]`)?.focus();
    }
  };

  return (
    <div
      ref={ref}
      className="color-grid"
      role="radiogroup"
      aria-label={label}
      data-grid-keys=""
      style={{ ["--cols" as string]: ACCOUNT_COLOR_COLUMNS }}
      onKeyDown={onKeyDown}
    >
      {ACCOUNT_COLORS.map((c, i) => {
        const on = i === current;
        return (
          <button
            key={c.hex}
            type="button"
            data-index={i}
            role="radio"
            aria-checked={on}
            aria-label={c.name}
            title={c.name}
            tabIndex={i === focus ? 0 : -1}
            className={`acc-swatch t-${accountTone(c.hex)}` + (on ? " selected" : "")}
            onFocus={() => setFocus(i)}
            onClick={() => onPick(c.hex)}
          >
            {on && <Icon name="check" size="xs" />}
          </button>
        );
      })}
    </div>
  );
}

/** The palette as a grid submenu: spread into a menu item ({ label: "Color", ...colorSubmenu(…) }). */
export function colorSubmenu(value: string | null | undefined, onPick: (hex: string) => void): Pick<MenuItem, "submenu" | "submenuGrid"> {
  return {
    submenuGrid: ACCOUNT_COLOR_COLUMNS,
    submenu: () =>
      ACCOUNT_COLORS.map((c) => ({
        label: c.name,
        lead: <ColorSwatch hex={c.hex} selected={same(value, c.hex)} />,
        checked: same(value, c.hex),
        onSelect: () => onPick(c.hex),
      })),
  };
}
