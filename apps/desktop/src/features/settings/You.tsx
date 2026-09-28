// Settings → You: your photo, the same across every account. Shown in the
// sidebar footer, the Floe bar, your own person card and wherever your address
// appears with an avatar. Your name is each account's Google profile name
// (lib/me.ts meName). Display-only: mail you send keeps each Google account's
// own name. OWNER: native.
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, asCommandError } from "../../lib/api";
import { meName, useMePhoto } from "../../lib/me";
import { AccountAvatar, Avatar, AccountBar, accountName } from "../../components/Identity";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { Select } from "../../components/Select";
import { meta } from "../../app/store";
import { Section } from "./parts";
import "./you.css";

export function YouSection() {
  const photo = useMePhoto();
  const accounts = meta.use((m) => m.accounts);
  const name = meName(accounts);
  const [crop, setCrop] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const primary = accounts[0];
  const named = accounts.find((a) => a.displayName);

  const pick = (file: File | undefined) => {
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => typeof reader.result === "string" && setCrop(reader.result);
    reader.onerror = () => toast({ tone: "error", message: "Couldn't read that file" });
    reader.readAsDataURL(file);
  };

  const run = async (what: string, f: () => Promise<unknown>, done: string) => {
    setBusy(what);
    try {
      await f();
      toast({ message: done });
    } catch (e) {
      toast({ tone: "error", message: asCommandError(e).message });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Section id="you" icon="user" title="You">
      <div className="me-card">
        <button
          className="me-photo-btn"
          title="Choose a photo"
          aria-label="Choose a photo"
          onClick={() => fileRef.current?.click()}
        >
          {primary ? (
            <Avatar person={{ name, email: primary.email }} size="2xl" tone="gray" photo={false} />
          ) : (
            <span className="avatar avatar-2xl t-gray">
              <Icon name="user" />
            </span>
          )}
          <span className="me-photo-edit" aria-hidden="true">
            <Icon name="image" size="xs" />
          </span>
        </button>
        <div className="me-card-text">
          <div className="me-card-name">{name ?? primary?.email ?? "You"}</div>
          {name && named && <div className="st-muted truncate">{named.email}</div>}
          <div className="me-photo-actions">
            <button className="btn btn-secondary btn-sm" onClick={() => fileRef.current?.click()}>
              {photo ? "Change photo…" : "Choose photo…"}
            </button>
            {photo && (
              <button
                className="btn btn-ghost btn-sm"
                disabled={busy !== null}
                onClick={() => void run("clear", api.clearMePhoto, "Photo removed")}
              >
                Remove
              </button>
            )}
          </div>
        </div>
        <input
          ref={fileRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          hidden
          onChange={(e) => {
            pick(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </div>

      {accounts.length > 0 && (
        <div className="setting-row setting-tall me-google">
          <div>
            <span className="setting-label">Use your Google profile photo</span>
            <p className="st-muted">Copied once from the account you pick. Uses the profile access Penguin already has.</p>
          </div>
          <div className="me-google-pick">
            {busy !== null && busy !== "clear" && <span className="st-spinner st-spinner-sm" aria-label="Loading" />}
            <Select
              className="me-google-select"
              label="Copy your photo from a Google account"
              value=""
              placeholder="Choose an account…"
              disabled={busy !== null}
              options={accounts.map((a) => ({
                value: a.id,
                label: accountName(a, accounts),
                lead: (
                  <>
                    <AccountAvatar account={a} accounts={accounts} />
                    <AccountBar color={a.color} />
                  </>
                ),
                hint: a.email,
              }))}
              onChange={(id) => void run(id, () => api.setMePhotoFromGoogle(id), "Photo updated from Google")}
            />
          </div>
        </div>
      )}

      <p className="st-muted settings-note">
        <Icon name="info" size="xs" /> Your photo is shown to you only, on this Mac. Your name is your Google profile name, and mail
        you send uses each account's own name: change it in your Google Account.
      </p>
      {crop && <PhotoCropper src={crop} onClose={() => setCrop(null)} />}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Crop: drag to position, slider (or +/−) to zoom, inside a circle. The
// result is a 512 px square PNG; the backend re-encodes it at 256 px.
// ---------------------------------------------------------------------------
const VIEW = 240;
const OUT = 512;

function PhotoCropper({ src, onClose }: { src: string; onClose: () => void }) {
  const [img, setImg] = useState<HTMLImageElement | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const [saving, setSaving] = useState(false);
  const drag = useRef<{ x: number; y: number; px: number; py: number } | null>(null);
  const saveRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const i = new Image();
    i.onload = () => {
      if (Math.min(i.naturalWidth, i.naturalHeight) < 32) {
        toast({ tone: "error", message: "That image is too small" });
        onClose();
        return;
      }
      setImg(i);
      const s = VIEW / Math.min(i.naturalWidth, i.naturalHeight);
      setPos({ x: (VIEW - i.naturalWidth * s) / 2, y: (VIEW - i.naturalHeight * s) / 2 });
      saveRef.current?.focus();
    };
    i.onerror = () => {
      toast({ tone: "error", message: "Couldn't read that image. Use a PNG, JPEG, WebP or GIF." });
      onClose();
    };
    i.src = src;
  }, [src, onClose]);

  const scale = img ? (VIEW / Math.min(img.naturalWidth, img.naturalHeight)) * zoom : 1;
  const clamp = (p: { x: number; y: number }, s = scale) =>
    img
      ? {
          x: Math.min(0, Math.max(VIEW - img.naturalWidth * s, p.x)),
          y: Math.min(0, Math.max(VIEW - img.naturalHeight * s, p.y)),
        }
      : p;

  // Zoom around the center of the circle.
  const setZoomAt = (z: number) => {
    if (!img) return;
    const nz = Math.min(4, Math.max(1, z));
    const base = VIEW / Math.min(img.naturalWidth, img.naturalHeight);
    const s0 = base * zoom;
    const s1 = base * nz;
    const cx = (VIEW / 2 - pos.x) / s0;
    const cy = (VIEW / 2 - pos.y) / s0;
    setZoom(nz);
    setPos(clamp({ x: VIEW / 2 - cx * s1, y: VIEW / 2 - cy * s1 }, s1));
  };

  const save = async () => {
    if (!img) return;
    setSaving(true);
    try {
      const canvas = document.createElement("canvas");
      canvas.width = OUT;
      canvas.height = OUT;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("Canvas is unavailable");
      ctx.imageSmoothingQuality = "high";
      ctx.drawImage(img, -pos.x / scale, -pos.y / scale, VIEW / scale, VIEW / scale, 0, 0, OUT, OUT);
      const png = canvas.toDataURL("image/png").split(",")[1];
      await api.setMePhoto(png);
      toast({ message: "Photo updated" });
      onClose();
    } catch (e) {
      toast({ tone: "error", message: `Couldn't save the photo: ${asCommandError(e).message}` });
      setSaving(false);
    }
  };

  useEffect(() => {
    // Capture phase, ahead of Settings' own keys: Esc cancels only this.
    const onKey = (e: KeyboardEvent) => {
      const step = e.shiftKey ? 20 : 6;
      const moves: Record<string, [number, number]> = { ArrowLeft: [step, 0], ArrowRight: [-step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] };
      if (e.key === "Escape") onClose();
      else if (e.key === "+" || e.key === "=") setZoomAt(zoom + 0.1);
      else if (e.key === "-") setZoomAt(zoom - 0.1);
      else if (moves[e.key] && !(e.target instanceof HTMLInputElement)) {
        const [dx, dy] = moves[e.key];
        setPos((p) => clamp({ x: p.x + dx, y: p.y + dy }));
      } else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const onDown = (e: ReactPointerEvent) => {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY, px: pos.x, py: pos.y };
  };
  const onMove = (e: ReactPointerEvent) => {
    const d = drag.current;
    if (d) setPos(clamp({ x: d.px + e.clientX - d.x, y: d.py + e.clientY - d.y }));
  };

  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="st-confirm me-crop" role="dialog" aria-modal="true" aria-label="Crop your photo">
        <h3>Position your photo</h3>
        <div
          className="me-crop-view"
          style={{ width: VIEW, height: VIEW }}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={() => (drag.current = null)}
          onWheel={(e) => setZoomAt(zoom - e.deltaY * 0.002)}
        >
          {img && (
            <img
              src={src}
              alt=""
              draggable={false}
              style={{ width: img.naturalWidth * scale, height: img.naturalHeight * scale, transform: `translate(${pos.x}px, ${pos.y}px)` }}
            />
          )}
          <span className="me-crop-mask" aria-hidden="true" />
        </div>
        <label className="me-crop-zoom">
          <Icon name="minus" size="xs" />
          <input
            type="range"
            min={1}
            max={4}
            step={0.01}
            value={zoom}
            aria-label="Zoom"
            onChange={(e) => setZoomAt(Number(e.target.value))}
          />
          <Icon name="plus" size="xs" />
        </label>
        <p className="st-muted me-crop-help">Drag to move · scroll or +/− to zoom · arrow keys nudge</p>
        <div className="st-confirm-actions">
          <button className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button ref={saveRef} className="btn btn-primary" disabled={!img || saving} onClick={() => void save()}>
            {saving ? "Saving…" : "Save photo"}
          </button>
        </div>
      </div>
    </div>
  );
}
