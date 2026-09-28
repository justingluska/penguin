// Account profiles (Settings → Profiles): named groups of accounts, such as a
// company's four addresses. The active profile (ui.profileId) narrows the
// inbox, search, labels, sync and compose to its accounts. ⌃1–⌃9 switch
// profiles, ⌃0 returns to all accounts. The profiles themselves live in
// settings.json; which one is active is remembered on this machine only.
import { useMemo } from "react";
import type { Account, Label, Profile } from "../lib/types";
import { currentSettings, getSettings, subscribeSettings, updateSettings, useSetting } from "../lib/settings";
import { PROFILE_STORAGE_KEY, getUi, setUi, subscribeUi, useUi } from "../lib/ui";
import { isConsumerAddress } from "../lib/format";
import { meta } from "./store";
import { sameOrder } from "./accountOrder";

/** Mirrors MAX_PROFILES in src-tauri/src/settings.rs. */
export const MAX_PROFILES = 20;
/**
 * Colors new profiles are given, in order (nextProfileColor): eight spread
 * across the palette's bright row. Any palette color can be picked after
 * (ACCOUNT_COLORS in lib/themes.ts); profiles render like accounts, in their
 * actual color (lib/accountColor.ts).
 */
export const PROFILE_COLORS = ["#4F7CFF", "#B06AD9", "#2BA3B8", "#2FA37A", "#E3A13B", "#FF8B3D", "#E0685A", "#7C8A9E"];

/**
 * Profiles as shown: members narrowed to signed-in accounts, in the order of
 * `accounts` (meta.accounts is in the user's account order, so a profile's
 * sidebar list, its compose From menu and its first account follow the same
 * drag order as everything else).
 */
export function profilesFor(profiles: Profile[], accounts: Account[]): Profile[] {
  const pos = new Map(accounts.map((a, i) => [a.id, i]));
  return profiles.map((p) => {
    const ids = p.accountIds.filter((id) => pos.has(id)).sort((a, b) => pos.get(a)! - pos.get(b)!);
    return sameOrder(ids, p.accountIds) ? p : { ...p, accountIds: ids };
  });
}

export function getProfiles(): Profile[] {
  return profilesFor(currentSettings().profiles, meta.get().accounts);
}

export function useProfiles(): Profile[] {
  const profiles = useSetting("profiles");
  const accounts = meta.use((m) => m.accounts);
  return useMemo(() => profilesFor(profiles, accounts), [profiles, accounts]);
}

export function activeProfile(): Profile | null {
  const id = getUi().profileId;
  return id ? getProfiles().find((p) => p.id === id) ?? null : null;
}

export function useActiveProfile(): Profile | null {
  const id = useUi((s) => s.profileId);
  const profiles = useProfiles();
  return id ? profiles.find((p) => p.id === id) ?? null : null;
}

/** Switch to a profile (null = all accounts); drops any single-account filter. */
export function switchProfile(id: string | null) {
  setUi({ profileId: id, accountFilter: null, threadOpen: false });
}

/** Unread inbox threads across a set of accounts (null = all), from the label counts. */
export function inboxUnread(labels: Label[], accountIds: string[] | null): number {
  let n = 0;
  for (const l of labels) if (l.id === "INBOX" && (!accountIds || accountIds.includes(l.accountId))) n += l.unreadCount ?? 0;
  return n;
}

/** Accounts for pickers: the active profile's accounts first (in account order), then the rest. */
export function profileFirst(accounts: Account[], profile: Profile | null = activeProfile()): Account[] {
  if (!profile || profile.accountIds.length === 0) return accounts;
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const first = profile.accountIds.map((id) => byId.get(id)).filter((a): a is Account => !!a);
  return [...first, ...accounts.filter((a) => !profile.accountIds.includes(a.id))];
}

/** Account a new message comes from: the filtered account, else the profile's first. */
export function preferredFromAccount(): string | null {
  return getUi().accountFilter ?? activeProfile()?.accountIds[0] ?? null;
}

// ---------------------------------------------------------------------------
// Editing (Settings → Profiles). Every edit writes the whole ordered list.
// ---------------------------------------------------------------------------
export function saveProfiles(profiles: Profile[]): Promise<unknown> {
  return updateSettings({ profiles: profiles.slice(0, MAX_PROFILES) });
}

export function newProfileId(): string {
  return `p-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

/** First palette color no profile uses yet (wraps around). */
export function nextProfileColor(profiles: Profile[]): string {
  const used = new Set(profiles.map((p) => p.color.toLowerCase()));
  return PROFILE_COLORS.find((c) => !used.has(c.toLowerCase())) ?? PROFILE_COLORS[profiles.length % PROFILE_COLORS.length];
}

// Common words for splitting a joined domain into a readable name
// ("bluewhale" → "Blue Whale"). A miss just capitalizes the label, and
// names are editable, so this only needs to cover the usual suspects.
const WORDS = new Set(
  (
    "the a an and of for my our your go get try use hey hi hello team work works labs lab studio studios group co company corp inc " +
    "global media digital tech systems solutions services consulting partners capital ventures ventures health care law legal " +
    "hire hiring talent jobs people staff agency market marketing sales data cloud soft software apps app web net mail " +
    "home house land real estate property pay bank money fund finance invest trade shop store goods food coffee " +
    "green blue red gold silver black white north south east west bright smart first one new true good big little " +
    "penguin fox bear wolf lion eagle hawk owl bird tiger panda whale shark rocket star sun moon sky sea ocean river lake " +
    "mountain rock stone wood tree leaf pine oak harbor bay port bridge gate tower city town street park garden farm " +
    "light fire water air earth space time life love health mind body design build maker craft code dev learn school"
  ).split(" "),
);

function splitWords(label: string): string[] | null {
  // Fewest-words segmentation; every piece must be a known word.
  const n = label.length;
  const best: (string[] | null)[] = Array(n + 1).fill(null);
  best[0] = [];
  for (let i = 1; i <= n; i++) {
    for (let j = Math.max(0, i - 12); j < i; j++) {
      const prev = best[j];
      const w = label.slice(j, i);
      if (prev && WORDS.has(w) && (!best[i] || prev.length + 1 < best[i]!.length)) best[i] = [...prev, w];
    }
  }
  return best[n];
}

/** "bluewhale.example" → "Blue Whale", "harbor-labs.example" → "Harbor Labs", "linden.example" → "Linden". */
export function nameForDomain(domain: string): string {
  const first = domain.split(".")[0].toLowerCase();
  const parts = first.split(/[-_]+/).filter(Boolean).flatMap((p) => (p.length > 3 ? splitWords(p) ?? [p] : [p]));
  return parts.map((w) => w[0].toUpperCase() + w.slice(1)).join(" ");
}

/**
 * One profile per company domain among the accounts (gmail.com and other
 * personal providers stay unassigned). Domains already covered by an
 * existing profile (every account of the domain is in it) are skipped.
 */
export function suggestProfiles(accounts: Account[], existing: Profile[]): Profile[] {
  const byDomain = new Map<string, Account[]>();
  for (const a of accounts) {
    if (isConsumerAddress(a.email)) continue;
    const domain = (a.email.split("@")[1] ?? "").toLowerCase();
    if (!domain) continue;
    byDomain.set(domain, [...(byDomain.get(domain) ?? []), a]);
  }
  const out: Profile[] = [];
  const all = [...existing];
  for (const [domain, members] of byDomain) {
    const ids = members.map((a) => a.id);
    if (existing.some((p) => ids.every((id) => p.accountIds.includes(id)))) continue;
    const p: Profile = {
      id: newProfileId() + out.length,
      name: nameForDomain(domain),
      color: members[0].color || nextProfileColor(all),
      accountIds: ids,
      emoji: null,
    };
    out.push(p);
    all.push(p);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Persistence of the active profile
// ---------------------------------------------------------------------------
/** Remember the active profile locally, and drop it if the profile is deleted. Called once by App. */
export function installProfiles(): () => void {
  let last = getUi().profileId;
  const offUi = subscribeUi(() => {
    const id = getUi().profileId;
    if (id === last) return;
    last = id;
    try {
      if (id) localStorage.setItem(PROFILE_STORAGE_KEY, id);
      else localStorage.removeItem(PROFILE_STORAGE_KEY);
    } catch {
      // Storage unavailable: the choice just isn't remembered across launches.
    }
  });
  let offSettings = () => {};
  let alive = true;
  // Before settings load the profile list reads as empty, so only prune after.
  void getSettings().then(() => {
    if (!alive) return;
    const prune = () => {
      const id = getUi().profileId;
      if (id && !currentSettings().profiles.some((p) => p.id === id)) setUi({ profileId: null });
    };
    prune();
    offSettings = subscribeSettings(prune);
  });
  return () => {
    alive = false;
    offUi();
    offSettings();
  };
}
