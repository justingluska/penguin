// Settings → Accounts: an account's nickname (inline rename) and color
// (a popover with the palette grid, components/ColorGrid.tsx). OWNER: themes. Saved with update_account; the store's
// account list is patched in place so every surface renames at once.
//
// Settings closes on Esc from a window capture listener; while the rename
// field or the color popover is open it carries [data-esc-owner], which that
// listener leaves alone so Esc cancels here instead.
import { useEffect, useRef, useState } from "react";
import type { Account } from "../../lib/types";
import { MAX_NICKNAME, patchAccount } from "../../app/accountActions";
import { AccountDot, accountName } from "../../components/Identity";
import { ColorGrid } from "../../components/ColorGrid";
import { Icon } from "../../components/Icon";
import { useDismiss } from "../../lib/dismiss";


/**
 * A color dot that opens the palette grid (accounts here, profiles in
 * Profiles.tsx). Focus moves into the grid on open and back on Esc.
 */
export function ColorPicker({
  color,
  label,
  className,
  onPick,
}: {
  color: string;
  /** Names the button ("Color for sam@…"). */
  label: string;
  className?: string;
  onPick: (hex: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useDismiss(open, () => setOpen(false), [ref]);
  useEffect(() => {
    if (open) ref.current?.querySelector<HTMLButtonElement>(".acc-swatch[tabindex='0']")?.focus();
  }, [open]);
  const close = () => {
    setOpen(false);
    ref.current?.querySelector<HTMLButtonElement>(".acc-color-btn")?.focus();
  };
  return (
    <span className={"acc-color-wrap" + (className ? " " + className : "")} ref={ref} data-esc-owner={open ? "" : undefined}>
      <button className="acc-color-btn" aria-label={label} aria-expanded={open} title="Change color" onClick={() => setOpen((o) => !o)}>
        <AccountDot color={color} />
      </button>
      {open && (
        <div
          className="panel acc-color-pop"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            }
          }}
        >
          <ColorGrid
            value={color}
            label={label}
            onPick={(hex) => {
              close();
              if (hex.toLowerCase() !== color.toLowerCase()) onPick(hex);
            }}
          />
        </div>
      )}
    </span>
  );
}

/** The account's color dot; click for the palette. */
export function AccountColorPicker({ account }: { account: Account }) {
  return (
    <ColorPicker
      className="account-color"
      color={account.color}
      label={`Color for ${account.email}`}
      onPick={(hex) => void patchAccount(account, { color: hex })}
    />
  );
}

/** Account title that turns into a text field; blank restores the derived name. */
export function AccountNickname({ account, accounts }: { account: Account; accounts: Account[] }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  // Enter/Esc end the edit; the blur that follows must not commit again.
  const ended = useRef(false);
  const derived = accountName({ ...account, nickname: null }, accounts);

  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const commit = () => {
    if (ended.current) return;
    ended.current = true;
    setEditing(false);
    const next = draft.trim().replace(/\s+/g, " ");
    if (next === (account.nickname ?? "")) return;
    void patchAccount(account, { nickname: next || null });
  };

  if (editing) {
    return (
      <form
        className="acc-nick-form"
        data-esc-owner=""
        onSubmit={(e) => {
          e.preventDefault();
          commit();
        }}
      >
        <input
          ref={inputRef}
          className="acc-nick-input"
          value={draft}
          maxLength={MAX_NICKNAME}
          placeholder={derived}
          aria-label={`Nickname for ${account.email}`}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              ended.current = true;
              setEditing(false);
            }
          }}
        />
      </form>
    );
  }
  return (
    <button
      className="account-title acc-nick"
      title="Rename (shown in the sidebar, switcher and lists)"
      onClick={() => {
        setDraft(account.nickname ?? "");
        ended.current = false;
        setEditing(true);
      }}
    >
      <span className="truncate">{accountName(account, accounts)}</span>
      <Icon name="compose" size="xs" className="acc-nick-ico" />
    </button>
  );
}
