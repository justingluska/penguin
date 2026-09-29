// Attachment cards and composer chips: the card menu's items and order
// (features/thread/attachmentMenu.ts), their keys, where the composer's
// preview reads each file (features/compose/previewSource.ts), and that a
// card drags only as its file, never as text (styles/app.css).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { attachmentCardMenu, cardKeyAction, type AttachmentMenuActions, type AttachmentMenuContext } from "../src/features/thread/attachmentMenu.ts";
import { previewSource, stepFile } from "../src/features/compose/previewSource.ts";
import { normalizeEntries, type MenuItem } from "../src/components/menuModel.ts";
import type { OutgoingAttachment } from "../src/lib/types.ts";

const picked: string[] = [];
const act: AttachmentMenuActions = {
  preview: () => picked.push("preview"),
  open: () => picked.push("open"),
  copy: () => picked.push("copy"),
  copyImage: () => picked.push("copyImage"),
  copyPath: () => picked.push("copyPath"),
  copyShareLink: () => picked.push("copyShareLink"),
  copyName: () => picked.push("copyName"),
  save: () => picked.push("save"),
  saveAs: () => picked.push("saveAs"),
  reveal: () => picked.push("reveal"),
};
const mac: AttachmentMenuContext = { image: false, nativeFiles: true, saved: false, openLabel: "Open", shareReady: true };

function labels(ctx: AttachmentMenuContext): string[] {
  return normalizeEntries(attachmentCardMenu(ctx, act)).map((e) => (e.type === "separator" ? "-" : String((e as MenuItem).label)));
}
function item(ctx: AttachmentMenuContext, label: string): MenuItem {
  const hit = normalizeEntries(attachmentCardMenu(ctx, act)).find((e) => e.type !== "separator" && (e as MenuItem).label === label);
  assert.ok(hit, `no ${label}`);
  return hit as MenuItem;
}

test("a file's card reads look, copy, keep: Preview, Open, Copy, Copy File Path, Copy Share Link, Save…, Show in Finder", () => {
  assert.deepEqual(labels({ ...mac, saved: true }), [
    "Preview",
    "Open",
    "-",
    "Copy",
    "Copy File Path",
    "Copy Share Link",
    "Copy File Name",
    "-",
    "Save to Downloads",
    "Save As…",
    "Show in Finder",
  ]);
  // Show in Finder only once this session saved it.
  assert.ok(!labels(mac).includes("Show in Finder"));
});

test("a picture's card adds Copy Image after Copy, and opens in Preview", () => {
  assert.deepEqual(labels({ image: true, nativeFiles: true, saved: false, openLabel: "Open in Preview", shareReady: true }), [
    "Preview",
    "Open in Preview",
    "-",
    "Copy",
    "Copy Image",
    "Copy File Path",
    "Copy Share Link",
    "Copy File Name",
    "-",
    "Save to Downloads",
    "Save As…",
  ]);
  // A file that isn't a picture never gets Copy Image.
  assert.ok(!labels(mac).includes("Copy Image"));
});

test("outside the Mac app: no file copy, path, Save As or Show in Finder, but still a share link", () => {
  assert.deepEqual(labels({ ...mac, nativeFiles: false, saved: true }), ["Preview", "Open", "-", "Copy Share Link", "Copy File Name", "-", "Save to Downloads"]);
});

test("Copy is the file (⌘C), Preview is Space, and each item runs its own action", () => {
  assert.equal(item(mac, "Copy").keys, "mod+c");
  assert.equal(item(mac, "Preview").keys, "space");
  picked.length = 0;
  for (const l of ["Preview", "Open", "Copy", "Copy File Path", "Copy Share Link", "Copy File Name", "Save to Downloads", "Save As…"]) item(mac, l).onSelect!();
  item({ ...mac, saved: true }, "Show in Finder").onSelect!();
  item({ ...mac, image: true }, "Copy Image").onSelect!();
  assert.deepEqual(picked, ["preview", "open", "copy", "copyPath", "copyShareLink", "copyName", "save", "saveAs", "reveal", "copyImage"]);
});

test("before share links are set up the item still shows, as Copy Share Link… with a Set up hint, for every file type", () => {
  for (const image of [false, true]) {
    const ctx = { ...mac, image, shareReady: false };
    const l = labels(ctx);
    assert.ok(l.includes("Copy Share Link…"), l.join(", "));
    assert.ok(!l.includes("Copy Share Link"));
    // In the copy group: after Copy File Path, before Copy File Name.
    assert.equal(l[l.indexOf("Copy Share Link…") - 1], "Copy File Path");
    assert.equal(l[l.indexOf("Copy Share Link…") + 1], "Copy File Name");
    const setUp = item(ctx, "Copy Share Link…");
    assert.equal(setUp.end, "Set up");
    assert.equal(setUp.text, "Copy Share Link", "type-ahead finds it by its name");
    picked.length = 0;
    setUp.onSelect!();
    assert.deepEqual(picked, ["copyShareLink"], "choosing it starts the setup, then the share");
  }
  assert.equal(item(mac, "Copy Share Link").end, undefined);
});

const key = (k: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) => ({
  key: k,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

test("keys on a focused card or chip: Space previews, ⌘C copies a card, ⌫ removes a chip", () => {
  const card = { copy: true };
  const chip = { remove: true };
  assert.equal(cardKeyAction(key(" "), card), "preview");
  assert.equal(cardKeyAction(key("Enter"), chip), "preview");
  assert.equal(cardKeyAction(key("c", { metaKey: true }), card), "copy");
  assert.equal(cardKeyAction(key("C", { metaKey: true }), card), "copy");
  // ⌘C on a chip, ⇧⌘C (copy conversation) or ⌥ variants aren't the card's.
  assert.equal(cardKeyAction(key("c", { metaKey: true }), chip), null);
  assert.equal(cardKeyAction(key("c", { metaKey: true, shiftKey: true }), card), null);
  assert.equal(cardKeyAction(key(" ", { altKey: true }), card), null);
  assert.equal(cardKeyAction(key("Backspace"), chip), "remove");
  assert.equal(cardKeyAction(key("Delete"), chip), "remove");
  assert.equal(cardKeyAction(key("Backspace"), card), null);
  assert.equal(cardKeyAction(key("Backspace", { metaKey: true }), chip), null);
  assert.equal(cardKeyAction(key("c"), card), null);
});

const added: OutgoingAttachment = { kind: "file", filename: "Floe plan.pdf", mimeType: "application/pdf", dataBase64: "JVBERi0=" };
const saved: OutgoingAttachment = { kind: "gmail", messageId: "draft-m2", attachmentId: "part:1", filename: "Budget.xlsx", mimeType: "application/vnd.ms-excel", size: 2048 };
const forwarded: OutgoingAttachment = { ...saved, messageId: "m9", attachmentId: "part:3", accountId: "juniper@floe.example" };
const inline: OutgoingAttachment = { kind: "file", filename: "shot.png", mimeType: "image/png", dataBase64: "iVBORw0KGgo=", contentId: "c1" };

test("the composer previews the exact file it will send", () => {
  // Just added: its own bytes.
  assert.deepEqual(previewSource(added, "sam@harbor.example"), { kind: "bytes", filename: "Floe plan.pdf", mimeType: "application/pdf", dataBase64: "JVBERi0=" });
  // A saved draft's: the stored part, in the draft's account.
  assert.deepEqual(previewSource(saved, "sam@harbor.example"), { kind: "stored", accountId: "sam@harbor.example", messageId: "draft-m2", attachmentId: "part:1" });
  // A forward from another account: that account's part.
  assert.deepEqual(previewSource(forwarded, "sam@harbor.example"), { kind: "stored", accountId: "juniper@floe.example", messageId: "m9", attachmentId: "part:3" });
});

test("←/→ in the composer's preview step over files, never inline images", () => {
  const list = [added, inline, saved, forwarded];
  assert.equal(stepFile(list, 0, 1), 2);
  assert.equal(stepFile(list, 2, -1), 0);
  assert.equal(stepFile(list, 3, 1), 3, "stays on the last");
  assert.equal(stepFile(list, 0, -1), 0, "stays on the first");
  // After removing, 0 steps to the file now at that index (or the first).
  assert.equal(stepFile(list, 2, 0), 2);
  assert.equal(stepFile(list, 1, 0), 0);
  assert.equal(stepFile([inline], 0, 0), -1, "no files left");
});

test("an attachment card drags as its file, never as text", () => {
  const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");
  const rule = (sel: string) => {
    const m = new RegExp(`${sel.replace(/[.*[\]"=]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(css);
    assert.ok(m, `no ${sel} rule`);
    return m[1];
  };
  // Nothing in a card can be selected, so WebKit never starts a selection drag…
  assert.match(rule(".file-btn"), /-webkit-user-select:\s*none/);
  assert.match(rule(".file-btn"), /(^|[^-])user-select:\s*none/);
  // …and nothing inside it (name, size, icon, button) drags on its own.
  assert.match(rule(".file-btn *"), /-webkit-user-drag:\s*none/);
  assert.match(rule('.file-btn[draggable="true"]'), /-webkit-user-drag:\s*element/);
  // The card is draggable (Mac app) and hands dragstart to the file drag.
  const view = readFileSync(new URL("../src/features/thread/ThreadView.tsx", import.meta.url), "utf8");
  const card = view.slice(view.indexOf("function AttachmentCard"), view.indexOf("function AttachmentList"));
  assert.match(card, /draggable=\{canDragFiles\}/);
  assert.match(card, /onDragStart=\{\(e\) => dragOut\(e, attachmentDragSource\(m, a\)\)\}/);
});
