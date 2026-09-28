// Unread pill for the sidebar's Inbox and account rows. Only inbox unread gets
// a pill: it's the count that asks for attention. Label counts stay plain,
// muted numbers so the pills stand out (styles/app.css, .badge-unread).
import { num } from "../../lib/format";

/** Past this the pill reads "99+"; the full count is in its label and tooltip. */
export const BADGE_MAX = 99;

export function badgeText(n: number): string {
  return n > BADGE_MAX ? `${BADGE_MAX}+` : String(n);
}

export function UnreadBadge({ n }: { n: number }) {
  if (!(n >= 1)) return null;
  const full = `${num(n)} unread`;
  return (
    <span className="count badge-unread" aria-label={full} title={n > BADGE_MAX ? full : undefined}>
      {badgeText(n)}
    </span>
  );
}
