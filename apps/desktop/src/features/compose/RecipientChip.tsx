// One recipient in To/Cc/Bcc. Shows the address next to the name when the
// name is missing or shared with another recipient (two "Alex"es), removes
// with ✕ or ⌫, and edits in place on double-click or Enter.
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { openPersonCard, PersonCardCompact, usePersonSummary } from "../people/PersonCard";
import { accountById } from "../../app/store";
import type { Address } from "../../lib/types";
import { Avatar, accountName } from "../../components/Identity";
import { Icon } from "../../components/Icon";
import { parseAddress } from "./draft";

/** Lowercased display names that more than one recipient uses. */
export function clashingNames(all: Address[]): Set<string> {
  const seen = new Map<string, Set<string>>();
  for (const a of all) {
    const n = a.name?.trim().toLowerCase();
    if (!n) continue;
    const emails = seen.get(n) ?? new Set<string>();
    emails.add(a.email.toLowerCase());
    seen.set(n, emails);
  }
  return new Set([...seen].filter(([, emails]) => emails.size > 1).map(([n]) => n));
}

function toText(a: Address): string {
  return a.name?.trim() ? `${a.name.trim()} <${a.email}>` : a.email;
}

export function RecipientChip({
  address,
  clash,
  onRemove,
  onReplace,
  onFocusInput,
  chipRef,
  onKeyNav,
}: {
  address: Address;
  /** Another recipient has the same name: show the address too. */
  clash: boolean;
  onRemove: () => void;
  onReplace: (a: Address) => void;
  /** Back to the typing box (Esc, or → past the last chip). */
  onFocusInput: () => void;
  chipRef?: (el: HTMLSpanElement | null) => void;
  /** ←/→ between chips. */
  onKeyNav: (dir: -1 | 1) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState("");
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const chipEl = useRef<HTMLSpanElement | null>(null);
  const [hover, setHover] = useState<DOMRect | null>(null);
  const showTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const name = address.name?.trim() || null;

  // Hover card: after 300 ms over the chip (or at once on keyboard focus);
  // it stays while the pointer moves onto it.
  function showSoon(delay: number) {
    if (hideTimer.current) clearTimeout(hideTimer.current);
    if (showTimer.current) clearTimeout(showTimer.current);
    showTimer.current = setTimeout(() => chipEl.current && setHover(chipEl.current.getBoundingClientRect()), delay);
  }
  function hideSoon() {
    if (showTimer.current) clearTimeout(showTimer.current);
    if (hideTimer.current) clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => setHover(null), 150);
  }
  useEffect(
    () => () => {
      if (showTimer.current) clearTimeout(showTimer.current);
      if (hideTimer.current) clearTimeout(hideTimer.current);
      if (clickTimer.current) clearTimeout(clickTimer.current);
    },
    [],
  );
  function openCard() {
    setHover(null);
    if (chipEl.current) openPersonCard(address, chipEl.current);
  }
  const showEmail = !name || clash;

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  function startEdit() {
    if (clickTimer.current) clearTimeout(clickTimer.current);
    setHover(null);
    setText(toText(address));
    setInvalid(false);
    setEditing(true);
  }

  function commit(): boolean {
    const a = parseAddress(text);
    if (!a) {
      setInvalid(true);
      return false;
    }
    setEditing(false);
    // Keep the known name when only the address was retyped.
    onReplace(a.name ? a : { name: a.email.toLowerCase() === address.email.toLowerCase() ? address.name : null, email: a.email });
    return true;
  }

  if (editing) {
    return (
      <span className={`rcpt rcpt-editing${invalid ? " rcpt-invalid" : ""}`}>
        <input
          ref={inputRef}
          className="rcpt-edit"
          value={text}
          size={Math.max(12, text.length)}
          onChange={(e) => {
            setText(e.target.value);
            setInvalid(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === "Tab") {
              if (commit()) {
                e.preventDefault();
                if (e.key === "Enter") onFocusInput();
              } else e.preventDefault();
            } else if (e.key === "Escape") {
              e.preventDefault(); // cancel the edit, not the composer
              e.stopPropagation();
              setEditing(false);
              onFocusInput();
            }
          }}
          onBlur={() => {
            if (!commit()) setEditing(false);
          }}
          aria-label={`Edit ${toText(address)}`}
          aria-invalid={invalid}
          spellCheck={false}
          autoComplete="off"
        />
      </span>
    );
  }

  function onKey(e: KeyboardEvent<HTMLSpanElement>) {
    if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      onRemove();
    } else if (e.key === "Enter") {
      e.preventDefault();
      startEdit();
    } else if (e.key === " ") {
      e.preventDefault();
      openCard();
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      onKeyNav(e.key === "ArrowLeft" ? -1 : 1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onFocusInput();
    }
  }

  return (
    <span
      ref={(el) => {
        chipEl.current = el;
        chipRef?.(el);
      }}
      className="rcpt"
      tabIndex={-1}
      aria-label={`${name ?? address.email}${name ? `, ${address.email}` : ""}. Space for details, Enter to edit, Delete to remove.`}
      onClick={(e) => {
        // Wait out a possible double-click (which edits) before opening the card.
        if (e.detail !== 1) return;
        if (clickTimer.current) clearTimeout(clickTimer.current);
        clickTimer.current = setTimeout(openCard, 220);
      }}
      onDoubleClick={startEdit}
      onKeyDown={onKey}
      onMouseEnter={() => showSoon(300)}
      onMouseLeave={hideSoon}
      onFocus={() => showSoon(0)}
      onBlur={hideSoon}
      onMouseDown={(e) => {
        // A double-click edits; don't let its first click move focus around.
        if (e.detail > 1) e.preventDefault();
      }}
    >
      <Avatar person={address} size="xs" />
      {name ?? address.email}
      {showEmail && name && <span className="rcpt-email">{address.email}</span>}
      <button
        className="rcpt-x"
        tabIndex={-1}
        aria-label={`Remove ${name ?? address.email}`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={(e) => {
          e.stopPropagation();
          onRemove();
        }}
      >
        <Icon name="x" size="2xs" />
      </button>
      {hover &&
        createPortal(
          <div
            className="panel rcpt-hover"
            style={{ top: hover.bottom + 6, left: Math.min(hover.left, window.innerWidth - 336) }}
            onMouseEnter={() => hideTimer.current && clearTimeout(hideTimer.current)}
            onMouseLeave={hideSoon}
            onClick={(e) => e.stopPropagation()}
          >
            <PersonCardCompact person={address} />
            <UsualAccount email={address.email} />
            <div className="rcpt-hover-foot faint">Click for more · double-click to edit</div>
          </div>,
          document.body,
        )}
    </span>
  );
}

/** "Usually from Northwind": the account that writes to them most. */
function UsualAccount({ email }: { email: string }) {
  const { data } = usePersonSummary(email);
  const usual = data?.accounts.map((a) => accountById(a.accountId)).find(Boolean);
  if (!usual) return null;
  return (
    <div className="rcpt-hover-usual">
      <i className="dot dot-sm" style={{ background: usual.color }} />
      Usually from <span className="emph">{accountName(usual)}</span>
    </div>
  );
}
