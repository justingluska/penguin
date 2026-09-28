// Settings → Profiles: group accounts by company or role. Create, rename,
// recolor, add an emoji, reorder (drag or ↑/↓), assign accounts (checkboxes,
// or drag an account chip onto a profile), delete, and a one-click "Suggest
// profiles by domain". Every edit saves the whole ordered list through
// lib/settings.ts. OWNER: settings agent (profiles feature).
import { useState, type DragEvent, type KeyboardEvent } from "react";
import type { Account, Profile } from "../../lib/types";
import { asCommandError } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { Keys } from "../../components/Kbd";
import { AccountAvatar, AccountDot, accountName } from "../../components/Identity";
import { toast } from "../../components/Toast";
import { meta } from "../../app/store";
import { PROFILE_MOD, profileKeys } from "../../app/shortcuts";
import {
  MAX_PROFILES,
  newProfileId,
  nextProfileColor,
  saveProfiles,
  suggestProfiles,
  switchProfile,
  useProfiles,
} from "../../app/profiles";
import { ProfileTile } from "../sidebar/Sidebar";
import { ConfirmDialog } from "./parts";
import { ColorPicker } from "./AccountIdentity";
import { Section } from "./parts";

const ACCOUNT_MIME = "application/x-penguin-account";
const PROFILE_MIME = "application/x-penguin-profile";

function save(profiles: Profile[]) {
  saveProfiles(profiles).catch((e) =>
    toast({ tone: "error", message: `Couldn't save profiles: ${asCommandError(e).message}` }),
  );
}

/** First grapheme of what was typed, or null; letters and digits aren't an emoji. */
function toEmoji(raw: string): string | null {
  const text = raw.trim();
  if (!text) return null;
  // Intl.Segmenter keeps multi-code-point emoji (👩‍💻, flags) whole; WebKit has it.
  type Segmenter = new (l?: string, o?: { granularity: "grapheme" }) => { segment(s: string): Iterable<{ segment: string }> };
  const Seg = (Intl as unknown as { Segmenter?: Segmenter }).Segmenter;
  const first = Seg ? [...new Seg(undefined, { granularity: "grapheme" }).segment(text)][0]?.segment ?? "" : [...text][0];
  return first && !/[A-Za-z0-9]/.test(first) && [...first].length <= 8 ? first : null;
}

export function ProfilesSection() {
  const profiles = useProfiles();
  const accounts = meta.use((m) => m.accounts);
  const [confirming, setConfirming] = useState<Profile | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);

  const update = (id: string, patch: Partial<Profile>) => save(profiles.map((p) => (p.id === id ? { ...p, ...patch } : p)));
  const move = (from: number, to: number) => {
    if (to < 0 || to >= profiles.length || from === to) return;
    const next = [...profiles];
    const [p] = next.splice(from, 1);
    next.splice(to, 0, p);
    save(next);
  };
  const create = () => {
    if (profiles.length >= MAX_PROFILES) return;
    const p: Profile = { id: newProfileId(), name: `Profile ${profiles.length + 1}`, color: nextProfileColor(profiles), accountIds: [], emoji: null };
    save([...profiles, p]);
    // Focus the new name field once it renders.
    requestAnimationFrame(() => document.querySelector<HTMLInputElement>(`[data-profile-name="${p.id}"]`)?.select());
  };
  const suggest = () => {
    const room = MAX_PROFILES - profiles.length;
    const found = suggestProfiles(accounts, profiles).slice(0, room);
    if (found.length === 0) {
      toast({
        message: accounts.every((a) => profiles.some((p) => p.accountIds.includes(a.id)))
          ? "Every account is already in a profile"
          : "No new company domains to group. Personal addresses (gmail.com and similar) stay unassigned",
      });
      return;
    }
    save([...profiles, ...found]);
    toast({ message: `Added ${found.map((p) => p.name).join(", ")}` });
  };
  const remove = (p: Profile) => {
    setConfirming(null);
    save(profiles.filter((x) => x.id !== p.id));
    toast({ message: `Deleted ${p.name}. Its accounts and mail are unchanged` });
  };

  const unassigned = accounts.filter((a) => !profiles.some((p) => p.accountIds.includes(a.id)));

  const onCardDragOver = (e: DragEvent, id: string) => {
    const types = e.dataTransfer.types;
    if (!types.includes(ACCOUNT_MIME) && !types.includes(PROFILE_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = types.includes(ACCOUNT_MIME) ? "copy" : "move";
    if (dropTarget !== id) setDropTarget(id);
  };
  const onCardDrop = (e: DragEvent, target: Profile, index: number) => {
    e.preventDefault();
    setDropTarget(null);
    const accountId = e.dataTransfer.getData(ACCOUNT_MIME);
    if (accountId) {
      if (!target.accountIds.includes(accountId)) update(target.id, { accountIds: [...target.accountIds, accountId] });
      return;
    }
    const from = profiles.findIndex((p) => p.id === e.dataTransfer.getData(PROFILE_MIME));
    if (from >= 0) move(from, index);
  };

  return (
    <Section
      id="profiles"
      icon="briefcase"
      title="Profiles"
      badge={profiles.length ? <span className="badge t-gray">{profiles.length === 1 ? "1 profile" : `${profiles.length} profiles`}</span> : undefined}
    >
      {profiles.length === 0 ? (
        <div className="pf-empty">
          <div className="pf-empty-art" aria-hidden="true">
            <span className="acct-tile t-blue">G</span>
            <span className="acct-tile t-green">H</span>
            <span className="acct-tile t-violet">P</span>
          </div>
          <div className="grow min0">
            <strong>Group accounts by company or role</strong>
            <p className="st-muted">
              Put the addresses you use for one company in a profile, then switch with <Keys keys={`${PROFILE_MOD}+1`} /> to{" "}
              <Keys keys={`${PROFILE_MOD}+9`} />. The inbox, search, labels and sync narrow to that profile&rsquo;s accounts;{" "}
              <Keys keys={`${PROFILE_MOD}+0`} /> shows everything again. An account can be in more than one profile.
            </p>
          </div>
        </div>
      ) : (
        <>
          <div className="pf-tray" aria-label="Accounts">
            <span className="st-muted">Drag an account onto a profile, or tick it below:</span>
            {accounts.map((a) => (
              <span
                key={a.id}
                className="chip pf-chip"
                draggable
                title={`Drag ${a.email} onto a profile`}
                onDragStart={(e) => {
                  e.dataTransfer.setData(ACCOUNT_MIME, a.id);
                  e.dataTransfer.effectAllowed = "copy";
                }}
              >
                <AccountAvatar account={a} accounts={accounts} />
                <AccountDot color={a.color} size="sm" />
                {a.email}
              </span>
            ))}
          </div>
          <ol className="pf-list">
            {profiles.map((p, i) => (
              <ProfileCard
                key={p.id}
                profile={p}
                index={i}
                count={profiles.length}
                accounts={accounts}
                dropping={dropTarget === p.id}
                dragging={dragging === p.id}
                onUpdate={(patch) => update(p.id, patch)}
                onMove={(to) => move(i, to)}
                onDelete={() => setConfirming(p)}
                onDragStart={() => setDragging(p.id)}
                onDragEnd={() => {
                  setDragging(null);
                  setDropTarget(null);
                }}
                onDragOver={(e) => onCardDragOver(e, p.id)}
                onDragLeave={() => setDropTarget((t) => (t === p.id ? null : t))}
                onDrop={(e) => onCardDrop(e, p, i)}
              />
            ))}
          </ol>
          {unassigned.length > 0 && (
            <p className="st-muted pf-unassigned">
              Not in a profile: {unassigned.map((a) => a.email).join(", ")}. They still show under All accounts.
            </p>
          )}
        </>
      )}
      <div className="settings-note row-flex" data-setting="profile-actions">
        <button className="btn btn-ghost btn-sm" onClick={suggest} disabled={profiles.length >= MAX_PROFILES || accounts.length === 0}>
          <Icon name="sparkles" size="xs" />
          Suggest profiles by domain
        </button>
        <span className="grow" />
        <button
          className="btn btn-ghost btn-sm"
          onClick={create}
          disabled={profiles.length >= MAX_PROFILES}
          title={profiles.length >= MAX_PROFILES ? `Up to ${MAX_PROFILES} profiles` : undefined}
        >
          <Icon name="plus" size="xs" />
          New profile
        </button>
      </div>
      {confirming ? (
        <ConfirmDialog
          title={`Delete the ${confirming.name} profile?`}
          body="Only the grouping is removed. Its accounts, mail and settings stay as they are."
          confirm="Delete profile"
          onConfirm={() => remove(confirming)}
          onCancel={() => setConfirming(null)}
        />
      ) : null}
    </Section>
  );
}

function ProfileCard({
  profile: p,
  index,
  count,
  accounts,
  dropping,
  dragging,
  onUpdate,
  onMove,
  onDelete,
  onDragStart,
  onDragEnd,
  onDragOver,
  onDragLeave,
  onDrop,
}: {
  profile: Profile;
  index: number;
  count: number;
  accounts: Account[];
  dropping: boolean;
  dragging: boolean;
  onUpdate: (patch: Partial<Profile>) => void;
  onMove: (to: number) => void;
  onDelete: () => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDragOver: (e: DragEvent) => void;
  onDragLeave: () => void;
  onDrop: (e: DragEvent) => void;
}) {
  const [name, setName] = useState(p.name);
  const [emoji, setEmoji] = useState(p.emoji ?? "");
  // Follow saved changes made elsewhere (another window, a suggestion).
  const [seen, setSeen] = useState({ name: p.name, emoji: p.emoji });
  if (seen.name !== p.name || seen.emoji !== p.emoji) {
    setSeen({ name: p.name, emoji: p.emoji });
    setName(p.name);
    setEmoji(p.emoji ?? "");
  }
  const keys = profileKeys(index);

  const commitName = () => {
    const v = name.replace(/\s+/g, " ").trim();
    if (!v) setName(p.name);
    else if (v !== p.name) onUpdate({ name: v.slice(0, 40) });
  };
  const commitEmoji = () => {
    const v = toEmoji(emoji);
    setEmoji(v ?? "");
    if (v !== p.emoji) onUpdate({ emoji: v });
  };
  const enterCommits = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
  };
  const toggle = (id: string, on: boolean) =>
    onUpdate({ accountIds: on ? [...p.accountIds, id] : p.accountIds.filter((x) => x !== id) });

  return (
    <li
      className={"pf-card" + (dropping ? " is-drop" : "") + (dragging ? " is-dragging" : "")}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      aria-label={`${p.name} profile`}
    >
      <div className="pf-head">
        <span
          className="pf-grip"
          draggable
          title="Drag to reorder"
          aria-hidden="true"
          onDragStart={(e) => {
            e.dataTransfer.setData(PROFILE_MIME, p.id);
            e.dataTransfer.effectAllowed = "move";
            const card = e.currentTarget.closest(".pf-card");
            if (card) e.dataTransfer.setDragImage(card, 16, 16);
            onDragStart();
          }}
          onDragEnd={onDragEnd}
        >
          <Icon name="more" size="xs" />
        </span>
        <ProfileTile profile={{ ...p, emoji: toEmoji(emoji) }} />
        <input
          className="input pf-name"
          data-profile-name={p.id}
          value={name}
          maxLength={40}
          aria-label="Profile name"
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          onKeyDown={enterCommits}
          spellCheck={false}
        />
        <input
          className="input pf-emoji"
          value={emoji}
          placeholder="🙂"
          aria-label="Emoji (optional)"
          title="Emoji (optional)"
          onChange={(e) => setEmoji(e.target.value)}
          onBlur={commitEmoji}
          onKeyDown={enterCommits}
        />
        <ColorPicker className="pf-color" color={p.color} label={`${p.name} color`} onPick={(hex) => onUpdate({ color: hex })} />
        <span className="grow" />
        <span className="pf-keys" title={keys ? "Switch to this profile" : "Only the first nine profiles get a shortcut"}>
          {keys ? <Keys keys={keys} /> : <span className="st-muted">No key</span>}
        </span>
        <button
          className="btn btn-ghost btn-sm btn-icon"
          aria-label={`Move ${p.name} up`}
          title="Move up"
          disabled={index === 0}
          onClick={() => onMove(index - 1)}
        >
          <Icon name="up" size="xs" />
        </button>
        <button
          className="btn btn-ghost btn-sm btn-icon"
          aria-label={`Move ${p.name} down`}
          title="Move down"
          disabled={index === count - 1}
          onClick={() => onMove(index + 1)}
        >
          <Icon name="down" size="xs" />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon" aria-label={`Switch to ${p.name}`} title="Switch to this profile" onClick={() => switchProfile(p.id)}>
          <Icon name="right" size="xs" />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon account-remove" aria-label={`Delete ${p.name}`} title="Delete profile" onClick={onDelete}>
          <Icon name="trash" size="xs" />
        </button>
      </div>
      <div className="pf-accounts">
        {accounts.map((a) => {
          const on = p.accountIds.includes(a.id);
          return (
            <label key={a.id} className={"pf-acct" + (on ? " is-on" : "")} title={a.email}>
              <input type="checkbox" checked={on} onChange={(e) => toggle(a.id, e.target.checked)} />
              <AccountAvatar account={a} accounts={accounts} />
              <AccountDot color={a.color} size="sm" />
              <span className="truncate">{a.email}</span>
              <span className="faint pf-acct-name">{accountName(a, accounts)}</span>
            </label>
          );
        })}
        {p.accountIds.length === 0 && <span className="st-muted pf-hint">No accounts yet. Tick some, or drop an account here.</span>}
      </div>
    </li>
  );
}
