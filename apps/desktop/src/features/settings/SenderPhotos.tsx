// Sender photos (avatars agent): "Sender photos appear" under General
// (list / inside emails / both / off), and Privacy → Sender photos (which sources may be
// used, per-account "Connect contact photos", and "Clear photo cache").
// The backend honors every switch (src-tauri/src/avatars/).
import { useCallback, useEffect, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import type { AvatarPlacement, AvatarStatus, SenderPhotos } from "../../lib/types";
import { ago, bytes, num } from "../../lib/format";
import { resetAvatars } from "../../lib/avatars";
import { toast } from "../../components/Toast";
import { Avatar, accountName } from "../../components/Identity";
import { meta } from "../../app/store";
import { Choice, Switch } from "./parts";
import "./sender-photos.css";

function savePhotos(patch: Partial<SenderPhotos>, current: SenderPhotos) {
  updateSettings({ senderPhotos: { ...current, ...patch } }).catch((e) =>
    toast({ tone: "error", message: asCommandError(e).message }),
  );
}

const PLACEMENT_NOTE: Record<AvatarPlacement, string> = {
  list: "Next to senders in the message list.",
  message: "In message headers, next to the sender's name.",
  both: "In the message list and in message headers.",
  off: "Initials only. Penguin doesn't look up or download any photos.",
};

export function AvatarPlacementRow() {
  const placement = useSetting("avatarPlacement");
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">Sender photos appear</span>
        <p className="st-muted">{PLACEMENT_NOTE[placement]} Sources are in Privacy → Sender photos.</p>
      </div>
      <Choice
        label="Sender photos appear"
        value={placement}
        onChange={(avatarPlacement) =>
          updateSettings({ avatarPlacement }).catch((e) => toast({ tone: "error", message: asCommandError(e).message }))
        }
        options={[
          { value: "list", label: "Message list" },
          { value: "message", label: "Inside emails" },
          { value: "both", label: "Both" },
          { value: "off", label: "Off" },
        ]}
      />
    </div>
  );
}

const SOURCES: { key: "bimi" | "favicons" | "gravatar"; label: string; note: string }[] = [
  {
    key: "bimi",
    label: "Brand logos (BIMI)",
    note: "Verified logos companies publish for their mail. Only shown on mail Google authenticated. Penguin looks up the sender's domain in DNS and downloads the logo.",
  },
  {
    key: "favicons",
    label: "Company icons",
    note: "The icon of the sender's website, for authenticated mail. Fetched once a month per domain, so that company can see your IP address asked for it.",
  },
  {
    key: "gravatar",
    label: "Gravatar",
    note: "Looks senders up on Gravatar by a hash of their address. This tells Gravatar (Automattic) who emails you, so it's off by default.",
  },
];

export function SenderPhotosPrivacy() {
  const photos = useSetting("senderPhotos");
  const off = useSetting("avatarPlacement") === "off";
  const accounts = meta.use((m) => m.accounts);
  const [status, setStatus] = useState<AvatarStatus | null>(null);
  const [connecting, setConnecting] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.avatarStatus().then(setStatus, () => setStatus(null));
  }, []);
  useEffect(refresh, [refresh, photos.contacts]);

  async function connect(accountId: string) {
    setConnecting(accountId);
    try {
      setStatus(await api.connectContactPhotos(accountId));
      toast({ message: "Contact photos connected" });
    } catch (e) {
      const err = asCommandError(e);
      if (err.code !== "cancelled") toast({ tone: "error", message: err.message });
    } finally {
      setConnecting(null);
    }
  }

  async function clear() {
    try {
      setStatus(await api.clearAvatarCache());
      resetAvatars();
      toast({ message: "Photo cache cleared" });
    } catch (e) {
      toast({ tone: "error", message: asCommandError(e).message });
    }
  }

  return (
    <div className="setting-row setting-tall sp-block">
      <div className="min0 grow">
        <span className="setting-label">Sender photos</span>
        <p className="st-muted">
          Penguin fetches these itself, from this Mac, and caches them. Never from inside an email, and never with anything
          about a message.
          {off && " They're off right now (General → Sender photos appear), so nothing is fetched."}
        </p>

        <div className="sp-source">
          <div className="min0">
            <span className="setting-label">Contact photos (Google)</span>
            <p className="st-muted">
              Photos of your Google contacts and people you've emailed. Needs read-only access to your contacts; connect each
              account once. Google may show a new consent screen.
            </p>
          </div>
          <Switch label="Contact photos" on={photos.contacts} onChange={(contacts) => savePhotos({ contacts }, photos)} />
        </div>
        {photos.contacts && (
          <ul className="sp-accounts">
            {accounts.map((a) => {
              const st = status?.accounts.find((x) => x.accountId === a.id);
              const busy = connecting === a.id;
              return (
                <li key={a.id}>
                  <Avatar person={{ name: accountName(a), email: a.email }} size="xs" photo={false} tone="gray" />
                  <span className="truncate sp-acct">{accountName(a)}</span>
                  <span className="st-muted truncate sp-state">
                    {st?.error
                      ? st.error
                      : st?.contactsGranted
                        ? st.syncedAt
                          ? `${num(st.photos)} photos · synced ${ago(st.syncedAt)}`
                          : "Connected · syncing…"
                        : "Not connected"}
                  </span>
                  {busy ? (
                    <button className="btn btn-ghost btn-sm" onClick={() => void api.cancelSignIn()}>
                      Cancel
                    </button>
                  ) : (
                    (!st?.contactsGranted || st.error) && (
                      <button className="btn btn-secondary btn-sm" disabled={connecting !== null} onClick={() => void connect(a.id)}>
                        {st?.contactsGranted ? "Reconnect" : "Connect"}
                      </button>
                    )
                  )}
                </li>
              );
            })}
          </ul>
        )}

        {SOURCES.map((src) => (
          <div className="sp-source" key={src.key}>
            <div className="min0">
              <span className="setting-label">{src.label}</span>
              <p className="st-muted">{src.note}</p>
            </div>
            <Switch label={src.label} on={photos[src.key]} onChange={(v) => savePhotos({ [src.key]: v }, photos)} />
          </div>
        ))}

        <div className="sp-source">
          <div className="min0">
            <span className="setting-label">Photo cache</span>
            <p className="st-muted">
              {status ? `${num(status.images)} ${status.images === 1 ? "image" : "images"} · ${bytes(status.bytes)}` : "On this Mac"}
            </p>
          </div>
          <button className="btn btn-secondary btn-sm" onClick={() => void clear()}>
            Clear photo cache
          </button>
        </div>
      </div>
    </div>
  );
}
