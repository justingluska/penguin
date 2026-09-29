// The right-click menu of an attachment card in a conversation, pictures
// and other files alike, in the order Finder's file menu reads: look at it
// (Preview, Open), copy it (the file, the picture, its path, a share link,
// its name), keep it (Save to Downloads, Save As…, Show in Finder). Pure
// (actions are passed in), so tests check the items and their order;
// threadMenus.tsx wires the actions.
import type { MenuEntries } from "../../components/menuModel.ts";
import { shareMenuItem } from "../share/model.ts";

export interface AttachmentMenuContext {
  /** A picture the image viewer shows: it adds Copy Image, and opens in Preview. */
  image: boolean;
  /** The Mac app: Copy (a file on the pasteboard), Copy File Path, Save As…, Show in Finder. */
  nativeFiles: boolean;
  /** This session saved the file (Show in Finder has one to show). */
  saved: boolean;
  /** "Open in Preview" for a picture on the Mac, else "Open". */
  openLabel: string;
  /** Share links are set up ("Copy Share Link"; else "Copy Share Link…", which sets them up first). */
  shareReady: boolean;
}

export interface AttachmentMenuActions {
  preview: () => void;
  open: () => void;
  /** The file itself, as Finder's Copy puts it on the pasteboard. */
  copy: () => void;
  /** Pictures: the picture's pixels (pastes into a document or chat as an image). */
  copyImage?: () => void;
  copyPath: () => void;
  /** Upload it to the user's storage and copy a link that expires (or set that up first). */
  copyShareLink: () => void;
  copyName: () => void;
  save: () => void;
  saveAs: () => void;
  reveal: () => void;
}

/** What a key does on a focused attachment card or composer chip. */
export type CardKeyAction = "preview" | "copy" | "remove" | null;

/**
 * Keys on a focused attachment: Space (and Enter) preview, like Quick Look;
 * ⌘C copies the file (conversation cards); ⌫ / Delete remove it (composer
 * chips). `can` says which of copy / remove the element offers.
 */
export function cardKeyAction(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean },
  can: { copy?: boolean; remove?: boolean },
): CardKeyAction {
  const mod = e.metaKey || e.ctrlKey;
  if (!mod && !e.altKey && (e.key === " " || e.key === "Enter")) return "preview";
  if (can.copy && mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "c") return "copy";
  if (can.remove && !mod && !e.altKey && (e.key === "Backspace" || e.key === "Delete")) return "remove";
  return null;
}

export function attachmentCardMenu(ctx: AttachmentMenuContext, act: AttachmentMenuActions): MenuEntries {
  return [
    // Space on a focused card, like Quick Look.
    { label: "Preview", icon: "eye", keys: "space", onSelect: act.preview },
    { label: ctx.image ? ctx.openLabel : "Open", icon: "external", onSelect: act.open },
    { type: "separator" },
    ctx.nativeFiles && { label: "Copy", icon: "copy", keys: "mod+c", onSelect: act.copy },
    ctx.image && !!act.copyImage && { label: "Copy Image", icon: "image", onSelect: act.copyImage },
    ctx.nativeFiles && { label: "Copy File Path", icon: "copy", onSelect: act.copyPath },
    // Any file type, and outside the Mac app too.
    shareMenuItem(ctx.shareReady, act.copyShareLink),
    { label: "Copy File Name", icon: "copy", onSelect: act.copyName },
    { type: "separator" },
    { label: "Save to Downloads", icon: "download", onSelect: act.save },
    ctx.nativeFiles && { label: "Save As…", text: "Save As", icon: "folder", onSelect: act.saveAs },
    ctx.nativeFiles && ctx.saved && { label: "Show in Finder", icon: "folder", onSelect: act.reveal },
  ];
}
