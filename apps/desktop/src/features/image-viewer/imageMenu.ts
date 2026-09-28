// The right-click menu for a picture: in the image viewer and on an image
// attachment's card in the thread. Pure (actions are passed in), so tests
// can check which items each kind of picture gets.
import type { MenuEntries } from "../../components/menuModel.ts";
import type { ViewerItem } from "./items.ts";

export interface ImageMenuActions {
  copy: () => void;
  copyAddress: () => void;
  save: () => void;
  saveAs: () => void;
  openInPreview: () => void;
  reveal: () => void;
  /** Save every picture of this picture's message (shown when `saveAllCount` ≥ 2). */
  saveAll?: () => void;
  /** Card only: open the viewer at this picture. */
  view?: () => void;
  /** Viewer only. */
  toggleZoom?: () => void;
  close?: () => void;
}

export interface ImageMenuContext {
  where: "viewer" | "card";
  /** This session saved the picture (Show in Finder then has a file to show). */
  saved: boolean;
  /** The Mac app: Save As… (system panel) and Show in Finder. */
  nativeFiles: boolean;
  /** Viewer: zoomed in past fit (the zoom item says "Zoom to Fit"). */
  zoomed?: boolean;
  /** Viewer: the picture's size is known (zooming works). */
  canZoom?: boolean;
  /** The picture failed to load: nothing to copy. */
  broken?: boolean;
  /** "Open in Preview" on the Mac, "Open" elsewhere. */
  openLabel: string;
  /** How many pictures its message has, when "Save All Images" applies (2 or more); else 0. */
  saveAllCount?: number;
}

export function imageMenu(it: ViewerItem, ctx: ImageMenuContext, act: ImageMenuActions): MenuEntries {
  const remote = it.kind === "body" && it.remote;
  const viewer = ctx.where === "viewer";
  return [
    !viewer && act.view && { label: "View", icon: "eye", onSelect: act.view },
    !viewer && { type: "separator" },
    { label: "Copy Image", icon: "copy", keys: viewer ? "mod+c" : undefined, disabled: ctx.broken ? "The image didn't load" : false, onSelect: act.copy },
    remote && { label: "Copy Image Address", icon: "link", onSelect: act.copyAddress },
    { type: "separator" },
    { label: "Save to Downloads", icon: "download", keys: viewer ? "mod+s" : undefined, onSelect: act.save },
    ctx.nativeFiles && { label: "Save As…", text: "Save As", icon: "folder", onSelect: act.saveAs },
    !!act.saveAll && (ctx.saveAllCount ?? 0) >= 2 && { label: `Save All Images (${ctx.saveAllCount})`, text: "Save All Images", icon: "download", onSelect: act.saveAll },
    { label: ctx.openLabel, icon: "external", keys: viewer ? "mod+o" : undefined, onSelect: act.openInPreview },
    ctx.nativeFiles && ctx.saved && { label: "Show in Finder", icon: "folder", onSelect: act.reveal },
    viewer && { type: "separator" },
    viewer &&
      act.toggleZoom && {
        label: ctx.zoomed ? "Zoom to Fit" : "Actual Size",
        icon: ctx.zoomed ? "shrink" : "expand",
        keys: "z",
        disabled: !ctx.canZoom,
        onSelect: act.toggleZoom,
      },
    viewer && act.close && { label: "Close", icon: "x", keys: "escape", onSelect: act.close },
  ];
}
