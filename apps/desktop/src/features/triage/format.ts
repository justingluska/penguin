// Follow up row text and groups. Pure, for tests/triage.test.ts.
const DAY = 86_400_000;

/** Whole days since `ms` (0 on the same day or in the future). */
export function daysSince(ms: number, now = Date.now()): number {
  return Math.max(0, Math.floor((now - ms) / DAY));
}

/** "Sent 5 days ago · no reply" (Follow up rows; `ms` = when it went out). */
export function waitingText(ms: number, now = Date.now()): string {
  const d = daysSince(ms, now);
  const when = d === 0 ? "today" : d === 1 ? "yesterday" : `${d} days ago`;
  return `Sent ${when} · no reply`;
}

/** Follow up's group headers (the list is oldest first). */
export function waitGroup(ms: number, now = Date.now()): string {
  const d = daysSince(ms, now);
  if (d >= 14) return "Waiting 2 weeks or more";
  if (d >= 7) return "Waiting over a week";
  return "Waiting under a week";
}
