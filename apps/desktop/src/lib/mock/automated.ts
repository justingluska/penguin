// Mirror of penguin-core `is_automated_address` (store_triage.rs) for the
// mock's Follow up: machines, not people, so waiting on them is pointless.
// Pure, for tests/triage.test.ts.
const WORDS = [
  "notification",
  "notifications",
  "notify",
  "mailer-daemon",
  "postmaster",
  "bounce",
  "bounces",
  "newsletter",
  "newsletters",
  "news",
  "updates",
  "digest",
  "alert",
  "alerts",
  "automated",
  "auto-confirm",
  "calendar-notification",
  "invitations",
  "unsubscribe",
];

export function isAutomatedAddress(email: string): boolean {
  const e = email.trim().toLowerCase();
  const at = e.lastIndexOf("@");
  const local = at < 0 ? e : e.slice(0, at);
  const domain = at < 0 ? "" : e.slice(at + 1);
  if (domain === "calendar.google.com" || domain.endsWith(".calendar.google.com")) return true;
  const squashed = local.replace(/[^a-z0-9]/g, "");
  if (squashed.includes("noreply") || squashed.includes("donotreply")) return true;
  return WORDS.some((w) => local === w || (local.startsWith(w) && "-+._".includes(local[w.length] ?? "\u0000")));
}
