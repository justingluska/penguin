// Mock sender avatars (avatars agent): illustrated portraits for some of the
// fictional people and simple marks for the fictional brands, as SVG data
// URLs. Like the real backend, the first lookup of a sender answers "none
// yet", then an avatars-changed event arrives and the image fades in.
import type { AccountPhotos, AvatarInfo, AvatarRequest, AvatarStatus } from "../types";
import { mockBackend, type MockHandler } from "./index";

const svg = (body: string) => `data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${body}</svg>`)}`;

/** A flat illustrated portrait: backdrop, shoulders, face, hair. */
function portrait(bg: [string, string], skin: string, hair: string, shirt: string, style: "short" | "long" | "bun" | "curly"): string {
  const hairShape = {
    short: `<path d="M19 27c0-10 6-15 13-15s13 5 13 15c-2-5-6-7-13-7s-11 2-13 7z" fill="${hair}"/>`,
    long: `<path d="M17 44c-2-18 3-32 15-32s17 14 15 32c-3-3-4-10-4-16-3-5-7-7-11-7s-8 2-11 7c0 6-1 13-4 16z" fill="${hair}"/>`,
    bun: `<circle cx="32" cy="10" r="6" fill="${hair}"/><path d="M19 28c0-10 6-15 13-15s13 5 13 15c-3-6-7-8-13-8s-10 2-13 8z" fill="${hair}"/>`,
    curly: `<g fill="${hair}"><circle cx="22" cy="21" r="6"/><circle cx="29" cy="15" r="7"/><circle cx="37" cy="15" r="7"/><circle cx="43" cy="22" r="6"/><circle cx="20" cy="28" r="4"/><circle cx="44" cy="29" r="4"/></g>`,
  }[style];
  return svg(
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${bg[0]}"/><stop offset="1" stop-color="${bg[1]}"/></linearGradient></defs>` +
      `<rect width="64" height="64" fill="url(#g)"/>` +
      `<path d="M10 64c2-12 11-18 22-18s20 6 22 18z" fill="${shirt}"/>` +
      `<rect x="28" y="37" width="8" height="10" rx="3" fill="${skin}"/>` +
      `<ellipse cx="32" cy="29" rx="11" ry="12.5" fill="${skin}"/>` +
      hairShape,
  );
}

const PHOTOS: Record<string, string> = {
  "priya@linden.example": portrait(["#fde2c8", "#f7b89a"], "#b9785a", "#2b1a14", "#3b5bdb", "long"),
  "marco@northwind.example": portrait(["#d3e4fd", "#9ec5f8"], "#e0ac86", "#5a3b25", "#1f2937", "short"),
  "dana@northwind.example": portrait(["#e9dcfb", "#c7b2f4"], "#f1c7a5", "#b5652b", "#0f766e", "bun"),
  "theo@harborlabs.example": portrait(["#d8f3e6", "#9fdcc0"], "#f0c29f", "#d9b56a", "#7c2d12", "short"),
  "grace.kim@alderpoint.example": portrait(["#fbe0ea", "#f3aac3"], "#f3d0b5", "#111827", "#475569", "long"),
  "ravi@northwind.example": portrait(["#fff1c7", "#fbd87a"], "#a86b4a", "#1c1917", "#155e75", "short"),
  "kofi@northwind.example": portrait(["#e0e7ff", "#b4c0fb"], "#7a4a2f", "#161412", "#b45309", "curly"),
  "ana@harborlabs.example": portrait(["#ffe4d6", "#fdb996"], "#e6b08e", "#7c2d12", "#4338ca", "curly"),
  "jonas@tidewater.example": portrait(["#e2e8f0", "#b8c4d4"], "#f2cdb0", "#a16207", "#166534", "short"),
};

/** Brand marks, shown on the white tile. */
const LOGOS: Record<string, string> = {
  "ledgerly.example": svg(`<rect width="64" height="64" rx="14" fill="#0f9d6b"/><path d="M22 16h8v26h14v7H22z" fill="#fff"/>`),
  "northline.example": svg(`<rect width="64" height="64" rx="14" fill="#1d3b8f"/><path d="M12 38l40-18-12 26-6-9z" fill="#fff"/><path d="M34 37l6 9" stroke="#9db4ff" stroke-width="2"/>`),
  "pinecrest.example": svg(`<rect width="64" height="64" rx="14" fill="#14532d"/><path d="M32 12l14 20h-7l9 12H16l9-12h-7z" fill="#bbf7d0"/><rect x="29" y="44" width="6" height="8" fill="#bbf7d0"/>`),
  "tidewater.example": svg(`<rect width="64" height="64" rx="14" fill="#0e7490"/><path d="M10 36c6-6 11-6 17 0s11 6 17 0 9-5 10-4v10c-2-1-5 0-10 4-6 6-11 6-17 0s-11-6-17 0z" fill="#fff"/><circle cx="44" cy="22" r="6" fill="#fde68a"/>`),
  "changelog.example": svg(`<rect width="64" height="64" rx="14" fill="#111827"/><path d="M20 22l-8 10 8 10M44 22l8 10-8 10M36 16l-8 32" stroke="#a5b4fc" stroke-width="5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>`),
  "brightfield.example": svg(`<rect width="64" height="64" rx="14" fill="#f59e0b"/><circle cx="32" cy="32" r="10" fill="#fff"/><g stroke="#fff" stroke-width="4" stroke-linecap="round"><path d="M32 10v6M32 48v6M10 32h6M48 32h6M16 16l4 4M44 44l4 4M48 16l-4 4M16 48l4-4"/></g>`),
  "harborlabs.example": svg(`<rect width="64" height="64" rx="14" fill="#fff"/><path d="M14 40h36l-6 10H20z" fill="#2563eb"/><path d="M31 12v26M31 14l14 20H31z" fill="#2563eb" stroke="#2563eb" stroke-width="2" stroke-linejoin="round"/>`),
};

/** Addresses at a brand domain that are people, not the brand. */
const isBrandSender = (email: string) => /^(no-?reply|billing|alerts|news|digest|offers|calendar|hello|community|rent)@/.test(email);

function answer(req: AvatarRequest): AvatarInfo {
  const email = req.email.trim().toLowerCase();
  const photo = PHOTOS[email];
  if (photo) return { email: req.email, kind: "photo", url: photo };
  const domain = email.split("@")[1] ?? "";
  const logo = LOGOS[domain];
  if (logo && isBrandSender(email) && req.authenticated !== false) return { email: req.email, kind: "logo", url: logo };
  return { email: req.email, kind: null, url: null };
}

const resolved = new Set<string>();
let pending: string[] = [];
let timer: ReturnType<typeof setTimeout> | null = null;

const accountPhotos = (): AccountPhotos[] => [
  { accountId: "acc-northwind", contactsGranted: true, syncedAt: Date.now() - 2 * 3600_000, photos: 412, error: null },
  { accountId: "acc-harbor", contactsGranted: false, syncedAt: null, photos: 0, error: null },
  { accountId: "acc-personal", contactsGranted: false, syncedAt: null, photos: 0, error: null },
];
let photosByAccount = accountPhotos();
const status = (): AvatarStatus => ({ accounts: photosByAccount, images: resolved.size, bytes: resolved.size * 7_400 });

/** The fictional Sam's own Google photo per account; Harbor Labs only has the letter avatar. */
const ACCOUNT_PHOTOS: Record<string, string | null> = {
  "acc-northwind": portrait(["#dbeafe", "#a5c8f5"], "#8a5a3c", "#1c1917", "#1e3a8a", "short"),
  "acc-harbor": null,
  "acc-personal": portrait(["#fde7c7", "#f6b980"], "#8a5a3c", "#1c1917", "#9a3412", "short"),
};

export const avatarHandlers: Record<string, MockHandler> = {
  // Like the real command, a first answer may wait on Google.
  account_photo: ({ accountId }) =>
    new Promise((resolve) => setTimeout(() => resolve(ACCOUNT_PHOTOS[accountId as string] ?? null), 250)),
  avatar_lookup: ({ requests }) =>
    (requests as AvatarRequest[]).map((r) => {
      const email = r.email.trim().toLowerCase();
      if (resolved.has(email)) return answer(r);
      // First sight: "unknown", resolved in the background.
      pending.push(email);
      timer ??= setTimeout(() => {
        timer = null;
        const emails = pending;
        pending = [];
        emails.forEach((e) => resolved.add(e));
        mockBackend.emit("penguin://avatars-changed", { emails, all: false });
      }, 350);
      return { email: r.email, kind: null, url: null };
    }),
  avatar_status: () => status(),
  connect_contact_photos: ({ accountId }) =>
    new Promise((resolve) =>
      setTimeout(() => {
        photosByAccount = photosByAccount.map((a) =>
          a.accountId === accountId ? { ...a, contactsGranted: true, syncedAt: Date.now(), photos: 87 } : a,
        );
        resolve(status());
      }, 900),
    ),
  clear_avatar_cache: () => {
    resolved.clear();
    mockBackend.emit("penguin://avatars-changed", { emails: [], all: true });
    return status();
  },
};
