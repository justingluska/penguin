// The right-click menu for a picture in the image viewer. (An attachment
// card in the thread has its own, features/thread/attachmentMenu.ts.) Pure
// (actions are passed in), so tests can check which items each kind of
// picture gets.
import type { MenuEntries } from "../../components/menuModel.ts";
import type { ViewerItem } from "./items.ts";
import { shareMenuItem } from "../share/model.ts";

export interface ImageMenuActions {
  copy: () => void;
  /** Copy Original Web Address: a remote picture's https address. */
  copyAddress: () => void;
  /** Save it if it isn't saved yet, then copy the file's path (for pasting into an agent or a terminal). */
  copyPath: () => void;
  /** Upload it to the user's storage and copy a link that expires (or set that up first). */
  copyShareLink?: () => void;
  save: () => void;
  saveAs: () => void;
  openInPreview: () => void;
  reveal: () => void;
  /** Save every picture of this picture's message (shown when `saveAllCount` ≥ 2). */
  saveAll?: () => void;
  toggleZoom?: () => void;
  close?: () => void;
}

export interface ImageMenuContext {
  /** This session saved the picture (Show in Finder then has a file to show). */
  saved: boolean;
  /** The Mac app: Save As… (system panel) and Show in Finder. */
  nativeFiles: boolean;
  /** Zoomed in past fit (the zoom item says "Zoom to Fit"). */
  zoomed?: boolean;
  /** The picture's size is known (zooming works). */
  canZoom?: boolean;
  /** The picture failed to load: nothing to copy. */
  broken?: boolean;
  /** "Open in Preview" on the Mac, "Open" elsewhere. */
  openLabel: string;
  /** Share links are set up ("Copy Share Link"; else "Copy Share Link…", which sets them up). */
  shareReady?: boolean;
  /** How many pictures its message has, when "Save All Images" applies (2 or more); else 0. */
  saveAllCount?: number;
}

export function imageMenu(it: ViewerItem, ctx: ImageMenuContext, act: ImageMenuActions): MenuEntries {
  const remote = it.kind === "body" && it.remote;
  const broken = ctx.broken ? "The image didn't load" : false;
  return [
    // The copy group: the pixels, then the file for an agent on this Mac,
    // a link for one elsewhere, and last the address the sender hosts it at.
    { label: "Copy Image", icon: "copy", keys: "mod+c", disabled: broken, onSelect: act.copy },
    ctx.nativeFiles && { label: "Copy File Path", icon: "copy", disabled: broken, onSelect: act.copyPath },
    !!act.copyShareLink && shareMenuItem(!!ctx.shareReady, act.copyShareLink, broken),
    remote && { label: "Copy Original Web Address", icon: "link", onSelect: act.copyAddress },
    { type: "separator" },
    { label: "Save to Downloads", icon: "download", keys: "mod+s", onSelect: act.save },
    ctx.nativeFiles && { label: "Save As…", text: "Save As", icon: "folder", onSelect: act.saveAs },
    !!act.saveAll && (ctx.saveAllCount ?? 0) >= 2 && { label: `Save All Images (${ctx.saveAllCount})`, text: "Save All Images", icon: "download", onSelect: act.saveAll },
    { label: ctx.openLabel, icon: "external", keys: "mod+o", onSelect: act.openInPreview },
    ctx.nativeFiles && ctx.saved && { label: "Show in Finder", icon: "folder", onSelect: act.reveal },
    { type: "separator" },
    act.toggleZoom && {
      label: ctx.zoomed ? "Zoom to Fit" : "Actual Size",
      icon: ctx.zoomed ? "shrink" : "expand",
      keys: "z",
      disabled: !ctx.canZoom,
      onSelect: act.toggleZoom,
    },
    act.close && { label: "Close", icon: "x", keys: "escape", onSelect: act.close },
  ];
}
