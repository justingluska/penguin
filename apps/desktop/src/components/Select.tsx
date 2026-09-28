// <Select>: the app's dropdown, in place of a native <select> (whose popup
// is a system menu that ignores the theme). A combobox button that opens a
// listbox through the shared menu host (ContextMenu.tsx), so it gets the
// same keyboard handling: ↑↓ Home End, type-ahead, Enter, Esc.
import { useRef, useState, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";
import { closeMenu, openMenu } from "./ContextMenu";

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  icon?: IconName;
  /** Custom leading visual (swatch, dot). */
  lead?: ReactNode;
  /** Muted text on the right of the option in the list. */
  hint?: string;
  disabled?: boolean | string;
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  className,
  disabled,
  placeholder = "Choose…",
}: {
  value: T;
  options: SelectOption<T>[];
  onChange: (v: T) => void;
  /** Accessible name (the visible label usually sits next to it). */
  label: string;
  className?: string;
  disabled?: boolean;
  placeholder?: string;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const cur = options.find((o) => o.value === value);

  const show = (keyboard: boolean) => {
    const el = ref.current;
    if (!el || disabled) return;
    setOpen(true);
    openMenu(
      el.getBoundingClientRect(),
      options.map((o) => ({
        label: o.label,
        icon: o.icon,
        lead: o.lead,
        end: o.hint,
        disabled: o.disabled,
        checked: o.value === value,
        onSelect: () => o.value !== value && onChange(o.value),
      })),
      {
        label,
        role: "listbox",
        minWidth: el.offsetWidth,
        activeIndex: Math.max(0, options.indexOf(cur as SelectOption<T>)),
        className: "cm-select",
        onClose: () => {
          setOpen(false);
          // Back on the trigger so the keyboard can carry on from here.
          if (keyboard) ref.current?.focus({ preventScroll: true });
        },
      },
      keyboard,
    );
  };

  return (
    <button
      ref={ref}
      type="button"
      role="combobox"
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-label={label}
      disabled={disabled}
      className={"select" + (open ? " is-open" : "") + (className ? " " + className : "")}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        if (open) closeMenu();
        else show(false);
      }}
      onClick={(e) => {
        // A click the keyboard made (Enter/Space as the button's default
        // action — e.g. inside Settings, whose key trap swallows keydown).
        if (e.detail === 0 && !open) show(true);
      }}
      onKeyDown={(e) => {
        if (["Enter", " ", "ArrowDown", "ArrowUp"].includes(e.key)) {
          e.preventDefault();
          e.stopPropagation();
          show(true);
        }
      }}
    >
      {cur?.lead ? <span className="select-lead">{cur.lead}</span> : cur?.icon ? <Icon name={cur.icon} size="xs" /> : null}
      <span className={"select-value truncate" + (cur ? "" : " is-placeholder")}>{cur?.label ?? placeholder}</span>
      <Icon name="updown" size="xs" className="select-chev" />
    </button>
  );
}
