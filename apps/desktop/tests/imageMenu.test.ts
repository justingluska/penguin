// The picture menu in the image viewer (features/image-viewer/imageMenu.ts):
// which items each kind of picture gets. Attachment cards: attachmentMenu.test.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { imageMenu, type ImageMenuActions, type ImageMenuContext } from "../src/features/image-viewer/imageMenu.ts";
import { attachmentItem, buildItems } from "../src/features/image-viewer/items.ts";
import { normalizeEntries, type MenuItem } from "../src/components/menuModel.ts";
import type { AttachmentMeta, MessageView } from "../src/lib/types.ts";

const PNG = "data:image/png;base64,iVBORw0KGgo=";
const REMOTE = "https://cdn.floe.example/launch/hero.jpg";

function msg(attachments: AttachmentMeta[] = []): MessageView {
  return { accountId: "acc", id: "m1", threadId: "t", date: 1, subject: "Launch", from: { name: "Juniper Hale", email: "juniper@floe.example" }, attachments } as MessageView;
}
const shot: AttachmentMeta = { id: "a1", filename: "screenshot.png", mimeType: "image/png", size: 1000, contentId: null, inline: false };

const picked: string[] = [];
const act: ImageMenuActions = {
  copy: () => picked.push("copy"),
  copyAddress: () => picked.push("copyAddress"),
  copyPath: () => picked.push("copyPath"),
  copyShareLink: () => picked.push("copyShareLink"),
  save: () => picked.push("save"),
  saveAs: () => picked.push("saveAs"),
  openInPreview: () => picked.push("openInPreview"),
  reveal: () => picked.push("reveal"),
  toggleZoom: () => picked.push("toggleZoom"),
  close: () => picked.push("close"),
};
const viewer: ImageMenuContext = { saved: false, nativeFiles: true, zoomed: false, canZoom: true, openLabel: "Open in Preview", shareReady: true };

function labels(entries: ReturnType<typeof imageMenu>): string[] {
  return normalizeEntries(entries).map((e) => (e.type === "separator" ? "-" : String((e as MenuItem).label)));
}
function item(entries: ReturnType<typeof imageMenu>, label: string): MenuItem {
  const hit = normalizeEntries(entries).find((e) => e.type !== "separator" && (e as MenuItem).label === label);
  assert.ok(hit, `no ${label}`);
  return hit as MenuItem;
}

const [embedded, remote] = buildItems([
  {
    message: msg(),
    body: [
      { index: 0, src: PNG, width: 40, height: 40, alt: "" },
      { index: 1, src: REMOTE, width: 800, height: 400, alt: "" },
    ],
  },
]);
const attached = attachmentItem(msg([shot]), shot);

test("an embedded picture: no image address (its data: URL is the picture itself), but a file path", () => {
  assert.equal(embedded.kind, "body");
  assert.deepEqual(labels(imageMenu(embedded, viewer, act)), [
    "Copy Image",
    "Copy File Path",
    "Copy Share Link",
    "-",
    "Save to Downloads",
    "Save As…",
    "Open in Preview",
    "-",
    "Actual Size",
    "Close",
  ]);
});

test("a remote picture adds Copy Original Web Address, last in the copy group", () => {
  assert.ok(remote.kind === "body" && remote.remote);
  const l = labels(imageMenu(remote, viewer, act));
  assert.deepEqual(l.slice(0, 5), ["Copy Image", "Copy File Path", "Copy Share Link", "Copy Original Web Address", "-"]);
  picked.length = 0;
  item(imageMenu(remote, viewer, act), "Copy Original Web Address").onSelect!();
  assert.deepEqual(picked, ["copyAddress"]);
});

test("Save All Images: only with an action and two or more pictures in the message", () => {
  const withSaveAll = { ...act, saveAll: () => picked.push("saveAll") };
  for (const n of [0, 1]) assert.ok(!labels(imageMenu(embedded, { ...viewer, saveAllCount: n }, withSaveAll)).some((l) => l.startsWith("Save All")));
  assert.ok(!labels(imageMenu(embedded, { ...viewer, saveAllCount: 3 }, act)).some((l) => l.startsWith("Save All")), "no action, no item");
  const l = labels(imageMenu(embedded, { ...viewer, saveAllCount: 3 }, withSaveAll));
  assert.equal(l[l.indexOf("Save As…") + 1], "Save All Images (3)");
  picked.length = 0;
  item(imageMenu(embedded, { ...viewer, saveAllCount: 3 }, withSaveAll), "Save All Images (3)").onSelect!();
  assert.deepEqual(picked, ["saveAll"]);
});

test("an attachment in the viewer, and Show in Finder only after a save", () => {
  assert.ok(!labels(imageMenu(attached, viewer, act)).includes("Copy Original Web Address"));
  assert.ok(!labels(imageMenu(attached, viewer, act)).includes("Show in Finder"));
  assert.ok(labels(imageMenu(attached, { ...viewer, saved: true }, act)).includes("Show in Finder"));
});

test("viewer keys, zoom label and state", () => {
  const m = imageMenu(remote, { ...viewer, zoomed: true }, act);
  assert.equal(item(m, "Copy Image").keys, "mod+c");
  assert.equal(item(m, "Save to Downloads").keys, "mod+s");
  assert.equal(item(m, "Open in Preview").keys, "mod+o");
  assert.equal(item(m, "Zoom to Fit").keys, "z");
  assert.equal(item(imageMenu(remote, { ...viewer, canZoom: false }, act), "Actual Size").disabled, true);
  assert.equal(item(imageMenu(remote, { ...viewer, broken: true }, act), "Copy Image").disabled, "The image didn't load");
  picked.length = 0;
  item(m, "Zoom to Fit").onSelect!();
  item(m, "Close").onSelect!();
  assert.deepEqual(picked, ["toggleZoom", "close"]);
});

test("outside the Mac app: no Save As or Show in Finder, and Open instead of Preview", () => {
  const l = labels(imageMenu(attached, { ...viewer, nativeFiles: false, saved: true, openLabel: "Open" }, act));
  assert.ok(!l.includes("Save As…"));
  assert.ok(!l.includes("Show in Finder"));
  assert.ok(l.includes("Open"));
});

test("Copy File Path picks the path action, and only where files are native", () => {
  picked.length = 0;
  item(imageMenu(attached, viewer, act), "Copy File Path").onSelect!();
  assert.deepEqual(picked, ["copyPath"]);
  assert.ok(!labels(imageMenu(attached, { ...viewer, nativeFiles: false }, act)).includes("Copy File Path"));
  assert.equal(item(imageMenu(remote, { ...viewer, broken: true }, act), "Copy File Path").disabled, "The image didn't load");
});

test("Copy Share Link is always offered; before setup it reads Copy Share Link… with a Set up hint", () => {
  const notSetUp = { ...viewer, shareReady: false };
  for (const it of [embedded, remote, attached]) {
    const l = labels(imageMenu(it, notSetUp, act));
    assert.ok(l.includes("Copy Share Link…"), `${it.kind}: ${l.join(", ")}`);
    assert.ok(!l.includes("Copy Share Link"));
  }
  const setUp = item(imageMenu(attached, notSetUp, act), "Copy Share Link…");
  assert.equal(setUp.text, "Copy Share Link", "type-ahead finds it by its name");
  assert.equal(setUp.end, "Set up");
  assert.equal(item(imageMenu(attached, viewer, act), "Copy Share Link").end, undefined);
  picked.length = 0;
  setUp.onSelect!();
  item(imageMenu(attached, viewer, act), "Copy Share Link").onSelect!();
  assert.deepEqual(picked, ["copyShareLink", "copyShareLink"]);
});

test("Copy Share Link doesn't need the Mac app's native files, and waits for the picture to load", () => {
  assert.ok(labels(imageMenu(attached, { ...viewer, nativeFiles: false }, act)).includes("Copy Share Link"));
  assert.equal(item(imageMenu(remote, { ...viewer, broken: true }, act), "Copy Share Link").disabled, "The image didn't load");
  const { copyShareLink: _, ...without } = act;
  assert.ok(!labels(imageMenu(attached, viewer, without)).some((l) => l.startsWith("Copy Share Link")), "no action, no item");
});
