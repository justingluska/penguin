// Identity primitives: account dots/badges, label chips/swatches, avatars.
// Accounts render their actual color (accountTone, lib/accountColor.ts); label
// colors map to the design system's theme-aware tones (toneForColor).
import { memo, useMemo, useState } from "react";
import "./avatar.css";
import "./me.css";
import type { Account, Address, Label } from "../lib/types";
import { accountLabel, initials, personTone, toneForColor } from "../lib/format";
import { accountTone } from "../lib/accountColor";
import { loadedUrls, useAvatar } from "../lib/avatars";
import { useMePhoto } from "../lib/me";
import { useAccountPhoto, useAccountPhotoAnswer } from "../lib/accountPhotos";
import { accountIdForEmail, isMe } from "../app/store";

export const AccountDot = memo(function AccountDot({
  color,
  size,
  className,
}: {
  color: string | null | undefined;
  size?: "sm";
  className?: string;
}) {
  let cls = `dot t-${accountTone(color)}`;
  if (size === "sm") cls += " dot-sm";
  if (className) cls += " " + className;
  return <i className={cls} />;
});

/** Thin vertical account-color bar (sidebar account list, switcher), Spark-style. */
export const AccountBar = memo(function AccountBar({ color }: { color: string | null | undefined }) {
  return <i className={`acct-bar t-${accountTone(color)}`} aria-hidden="true" />;
});

/**
 * How the UI names an account: its nickname ("Sam Work") when set, else the
 * label derived from the address ("Northwind"); two accounts that would get
 * the same derived label fall back to the address.
 */
export function accountName(a: Account, all?: Account[]): string {
  if (a.nickname) return a.nickname;
  const label = accountLabel(a);
  if (all && all.some((b) => b.id !== a.id && !b.nickname && accountLabel(b) === label)) return a.email;
  return label;
}

/** Tinted account badge ("● Northwind"), used in thread headers. */
export function AccountBadge({ account, label }: { account: Account; label?: string }) {
  return (
    <span className={`badge t-${accountTone(account.color)}`}>
      <i className="dot dot-sm" />
      {label ?? accountName(account)}
    </span>
  );
}

/** Neutral outlined label chip with a color swatch. */
export const LabelChip = memo(function LabelChip({ label }: { label: Pick<Label, "name" | "color"> }) {
  return (
    <span className={`badge lbl t-${toneForColor(label.color)}`} title={label.name}>
      <span className="lbl-name">{label.name}</span>
    </span>
  );
});

/** Square label swatch for the sidebar (labels are squares, accounts are circles). */
export function LabelSwatch({ color }: { color: string | null }) {
  return <span className={`label-sq t-${toneForColor(color)}`} />;
}

/**
 * A person or sender: the monogram, with their photo (round) or brand logo
 * (white rounded tile) faded in over it once known. The slot never changes
 * size, so nothing shifts. `photo={false}` skips the sender lookup (e.g.
 * yourself); `authenticated` is the message's senderAuthenticated when known.
 * Your own addresses show that account's own Google profile photo, so each
 * account (say a work persona) looks like itself; an account without one
 * shows your Settings → You photo when there is one.
 */
export const Avatar = memo(function Avatar({
  person,
  size,
  tone,
  photo = true,
  authenticated = null,
  className,
}: {
  person: Address;
  /** xs 18 · sm 22 · (default) 28 · md 32 · row 36 · lg 40 · xl 56 · 2xl 64 (contact card). */
  size?: "xs" | "sm" | "md" | "row" | "lg" | "xl" | "2xl";
  tone?: string;
  photo?: boolean;
  authenticated?: boolean | null;
  className?: string;
}) {
  const t = tone ?? personTone(person.email);
  const mePhoto = useMePhoto();
  const own = useAccountPhotoAnswer(isMe(person.email) ? accountIdForEmail(person.email) : null);
  // Yours: the account's photo; once it has none, the You photo; while the
  // account's answer is pending, the monogram (no flash of the other photo).
  const mine = isMe(person.email) && (own !== null || mePhoto !== null);
  const info = useAvatar(photo && !mine ? person.email : null, authenticated);
  const url = mine ? (own === undefined ? null : (own ?? mePhoto)) : (info?.url ?? null);
  const fade = useFadeIn(url);
  let cls = `avatar${size ? " avatar-" + size : ""} t-${t}${fade.cls}`;
  if (fade.shown && info?.kind === "logo") cls += " is-logo";
  if (className) cls += " " + className;
  return (
    <span className={cls} title={person.email}>
      {initials(person)}
      {fade.img}
    </span>
  );
});

/** The image faded in over the monogram once it loads; instant if already shown this session. */
function useFadeIn(url: string | null) {
  const [loaded, setLoaded] = useState<string | null>(null);
  // Already shown once this session (a row scrolled back into view): no fade.
  const instant = useMemo(() => url !== null && loadedUrls.has(url), [url]);
  const shown = url !== null && (loaded === url || instant);
  let cls = "";
  if (url) cls += " has-src";
  if (shown) cls += " has-img";
  if (instant) cls += " no-fade";
  const img = url && (
    <img
      key={url}
      src={url}
      alt=""
      draggable={false}
      decoding="async"
      onLoad={() => {
        loadedUrls.add(url);
        setLoaded(url);
      }}
    />
  );
  return { cls, shown, img };
}

/**
 * An account: its own Google profile photo (lib/accountPhotos.ts) faded in
 * over a monogram of its name in the account's color. Round, like people;
 * the account's color bar or dot still sits beside it where accounts are listed.
 */
export const AccountAvatar = memo(function AccountAvatar({
  account,
  accounts,
  size = "xs",
  className,
}: {
  account: Account;
  /** All accounts, so the monogram follows accountName's disambiguation. */
  accounts?: Account[];
  size?: "xs" | "sm" | "md";
  className?: string;
}) {
  const fade = useFadeIn(useAccountPhoto(account.id));
  const letter = accountName(account, accounts).trim().slice(0, 1).toUpperCase() || "?";
  return (
    <span className={`avatar avatar-${size} acct-av t-${accountTone(account.color)}${fade.cls}${className ? " " + className : ""}`} aria-hidden="true">
      {letter}
      {fade.img}
    </span>
  );
});
