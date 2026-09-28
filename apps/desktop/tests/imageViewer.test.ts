// Image viewer (features/image-viewer): the click bridge's request schema,
// which requests it honors (only pictures on screen in the frame that minted
// the nonce), which picture sources it shows, and what the viewer steps through.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  _resetFrames,
  framesInOrder,
  imageUrl,
  isViewableSrc,
  newNonce,
  parseImageRequest,
  registerFrame,
  resolveImageRequest,
  type ImgLike,
} from "../src/features/image-viewer/bridge.ts";
import { buildItems, canSaveAll, dataUrlBytes, frameBodyImages, isViewableAttachment, matchInlinePart, messageItems, nameFromUrl, onScreenMessageItems, saveAllItems } from "../src/features/image-viewer/items.ts";
import type { AttachmentMeta, MessageView } from "../src/lib/types.ts";

const N = "0123456789abcdef0123456789abcdef";
const PNG = "data:image/png;base64,iVBORw0KGgo=";

test("the request schema: our anchor href or the native event's {nonce, index}, nothing else", () => {
  assert.deepEqual(parseImageRequest(`penguin-image://open/${N}/0`), { nonce: N, index: 0 });
  assert.deepEqual(parseImageRequest(`penguin-image://open/${N}/9999`), { nonce: N, index: 9999 });
  assert.deepEqual(parseImageRequest({ nonce: N, index: 3 }), { nonce: N, index: 3 });
  assert.deepEqual(parseImageRequest({ index: 3, nonce: N }), { nonce: N, index: 3 });

  const badStrings = [
    "",
    `https://open/${N}/0`,
    `penguin-image://open/${N}/00`,
    `penguin-image://open/${N}/01`,
    `penguin-image://open/${N}/10000`,
    `penguin-image://open/${N}/-1`,
    `penguin-image://open/${N}/1.5`,
    `penguin-image://open/${N}/1/2`,
    `penguin-image://open/${N}/`,
    `penguin-image://open/${N}/1?x`,
    `penguin-image://open/${N}/1#x`,
    ` penguin-image://open/${N}/1`,
    `penguin-image://open/${N}/1\n`,
    `PENGUIN-IMAGE://open/${N}/1`,
    `penguin-image://open/${N.toUpperCase()}/1`,
    `penguin-image://open/${N.slice(1)}/1`,
    `penguin-image://open/${N}0/1`,
    `penguin-image://u@open/${N}/1`,
    `penguin-image://other/${N}/1`,
    `penguin-image://open/${N}/1${"0".repeat(80)}`,
  ];
  for (const s of badStrings) assert.equal(parseImageRequest(s), null, JSON.stringify(s));

  const badPayloads: unknown[] = [
    null,
    undefined,
    0,
    [N, 1],
    {},
    { nonce: N },
    { index: 1 },
    { nonce: N, index: "1" },
    { nonce: N, index: 1.5 },
    { nonce: N, index: -1 },
    { nonce: N, index: 10000 },
    { nonce: N, index: Number.NaN },
    { nonce: N.toUpperCase(), index: 1 },
    { nonce: `${N} `, index: 1 },
    { nonce: N, index: 1, src: "https://evil.example/x.png" },
    { nonce: N, index: 1, url: "javascript:alert(1)" },
    Object.assign(Object.create({ polluted: true }), { nonce: N, index: 1 }),
    new (class Req { nonce = N; index = 1; })(),
  ];
  for (const p of badPayloads) assert.equal(parseImageRequest(p), null, String(JSON.stringify(p)));
});

test("nonces are 128 random bits in lowercase hex, and anchors round-trip", () => {
  const seen = new Set(Array.from({ length: 200 }, newNonce));
  assert.equal(seen.size, 200);
  for (const n of seen) assert.match(n, /^[0-9a-f]{32}$/);
  const n = newNonce();
  assert.deepEqual(parseImageRequest(imageUrl(n, 42)), { nonce: n, index: 42 });
  assert.throws(() => imageUrl("nope", 1));
  assert.throws(() => imageUrl(n, 10000));
});

test("only raster data: images and plain https URLs are viewable sources", () => {
  for (const ok of [PNG, "data:image/jpeg;base64,/9j/4AAQ", "data:image/webp;base64,UklGRg==", "https://cdn.example/launch.png", "https://cdn.example/a.png?w=600"]) {
    assert.ok(isViewableSrc(ok), ok);
  }
  for (const bad of [
    "",
    null,
    42,
    "data:image/svg+xml;base64,PHN2Zz4=",
    "data:image/svg+xml,<svg onload=alert(1)>",
    "data:text/html;base64,PHNjcmlwdD4=",
    "data:image/png,rawbytes",
    "data:image/png;base64,<script>",
    "http://cdn.example/a.png",
    "https://user:pw@cdn.example/a.png",
    "javascript:alert(1)",
    "blob:tauri://localhost/1234",
    "avatar://localhost/abc.png",
    "file:///etc/passwd",
    "cid:hero@floe.example",
    "//cdn.example/a.png",
  ]) {
    assert.ok(!isViewableSrc(bad), String(bad));
  }
});

// --- the registry, with duck-typed stand-ins for the frame's DOM ---
function fakeImg(doc: object, src: string, loaded = true): ImgLike & { isConnected: boolean; currentSrc: string } {
  return { isConnected: true, ownerDocument: doc, complete: loaded, naturalWidth: loaded ? 1600 : 0, naturalHeight: loaded ? 900 : 0, currentSrc: src, getAttribute: (n) => (n === "src" ? src : n === "alt" ? "" : null) };
}
function msg(id: string, extra: Partial<MessageView> = {}): MessageView {
  return { accountId: "acc", id, threadId: "t", date: Number(id.replace(/\D/g, "")) || 0, subject: "Launch", from: { name: "Juniper Hale", email: "juniper@floe.example" }, attachments: [], ...extra } as MessageView;
}
function fakeFrame(images: ImgLike[], message = msg("m1")) {
  const doc = { name: "srcdoc" };
  const frame = { isConnected: true, contentDocument: doc as unknown };
  const nonce = newNonce();
  const off = registerFrame({ nonce, doc, frame, message: () => message, images });
  return { doc, frame, nonce, off, images };
}

test("a request opens only a picture the app decorated, in a frame still on screen", () => {
  _resetFrames();
  const doc = { name: "srcdoc" };
  const img = fakeImg(doc, PNG);
  const frame = { isConnected: true, contentDocument: doc as unknown };
  const nonce = newNonce();
  const off = registerFrame({ nonce, doc, frame, message: () => msg("m1"), images: [img] });

  const hit = resolveImageRequest({ nonce, index: 0 });
  assert.ok(hit);
  assert.equal(hit.src, PNG);
  assert.equal(hit.index, 0);
  assert.ok(resolveImageRequest(imageUrl(nonce, 0), { doc }), "the in-frame listener path, from its own document");

  // Forged or unknown sources.
  assert.equal(resolveImageRequest({ nonce: newNonce(), index: 0 }), null, "a nonce nobody minted");
  assert.equal(resolveImageRequest({ nonce, index: 1 }), null, "an index the app never assigned");
  assert.equal(resolveImageRequest({ nonce, index: 0, extra: 1 }), null, "an off-schema payload");
  assert.equal(resolveImageRequest(imageUrl(nonce, 0), { doc: { name: "another message" } }), null, "a click seen in another message's document");

  // A picture that isn't (or is no longer) what the frame shows.
  img.currentSrc = "data:image/svg+xml;base64,PHN2Zz4=";
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null, "a source the viewer won't show");
  img.currentSrc = PNG;
  img.isConnected = false;
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null, "a picture removed from the document");
  img.isConnected = true;

  // The frame went away or loaded another document (images toggled, message changed).
  frame.contentDocument = { name: "new srcdoc" };
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null);
  frame.contentDocument = doc;
  frame.isConnected = false;
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null);
  frame.isConnected = true;
  assert.ok(resolveImageRequest({ nonce, index: 0 }));

  off();
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null, "unregistered on unmount");
});

test("a remote picture opens only once it really loaded in the frame (its CSP and the image setting allowed it)", () => {
  _resetFrames();
  const doc = { name: "srcdoc" };
  const pending = fakeImg(doc, "https://cdn.example/hero.jpg", false);
  const frame = { isConnected: true, contentDocument: doc as unknown };
  const nonce = newNonce();
  registerFrame({ nonce, doc, frame, message: () => msg("m1"), images: [pending] });
  assert.equal(resolveImageRequest({ nonce, index: 0 }), null);
  Object.assign(pending, { complete: true, naturalWidth: 1200, naturalHeight: 600 });
  assert.equal(resolveImageRequest({ nonce, index: 0 })?.src, "https://cdn.example/hero.jpg");
  _resetFrames();
});

test("frames come back in on-screen order and unmounted ones are skipped", () => {
  _resetFrames();
  const a = fakeFrame([], msg("m1"));
  const b = fakeFrame([], msg("m2"));
  // compareDocumentPosition stand-ins: a precedes b.
  Object.assign(a.frame, { compareDocumentPosition: (o: unknown) => (o === b.frame ? 4 : 0) });
  Object.assign(b.frame, { compareDocumentPosition: (o: unknown) => (o === a.frame ? 2 : 0) });
  assert.deepEqual(framesInOrder().map((f) => f.message().id), ["m1", "m2"]);
  b.frame.isConnected = false;
  assert.deepEqual(framesInOrder().map((f) => f.message().id), ["m1"]);
  _resetFrames();
});

// --- what the viewer steps through ---
const att = (id: string, filename: string, mimeType: string, size: number, inline = false): AttachmentMeta => ({ id, filename, mimeType, size, contentId: inline ? `${id}@x.example` : null, inline });

test("image attachments the viewer shows: png, jpeg, gif, webp; never inline parts or other files", () => {
  assert.ok(isViewableAttachment(att("1", "shot.png", "image/png", 10)));
  assert.ok(isViewableAttachment(att("1", "photo.JPG", "application/octet-stream", 10)));
  assert.ok(isViewableAttachment(att("1", "anim", "image/gif", 10)));
  assert.ok(!isViewableAttachment(att("1", "IMG_0001.HEIC", "image/heic", 10)));
  assert.ok(!isViewableAttachment(att("1", "logo.svg", "image/svg+xml", 10)));
  assert.ok(!isViewableAttachment(att("1", "brief.pdf", "application/pdf", 10)));
});

test("an embedded picture keeps its inline part's name when the size identifies it", () => {
  // 12 base64 chars, one pad = 8 bytes.
  const src = "data:image/png;base64,iVBORw0KGgo=";
  assert.equal(dataUrlBytes(src), 8);
  const m = msg("m1", { attachments: [att("a", "hero.png", "image/png", 8, true), att("b", "other.png", "image/png", 9, true)] });
  assert.equal(matchInlinePart(m, src)?.filename, "hero.png");
  const twins = msg("m1", { attachments: [att("a", "a.png", "image/png", 8, true), att("b", "b.png", "image/png", 8, true)] });
  assert.equal(matchInlinePart(twins, src), null, "ambiguous: no guess");
  assert.equal(matchInlinePart(m, "https://cdn.example/hero.png"), null);
});

test("names from URLs never include the query and are cleaned", () => {
  assert.equal(nameFromUrl("https://cdn.example/img/launch%20art.png?utm=sam%40example"), "launch art.png");
  assert.equal(nameFromUrl("https://cdn.example/"), "");
  assert.equal(nameFromUrl("https://cdn.example/a%2F..%2Fb.png"), "a_.._b.png");
  assert.equal(nameFromUrl(PNG), "");
});

test("the viewer steps through each message's body pictures, then its image attachments, in thread order", () => {
  const m1 = msg("m1", { attachments: [att("hero", "hero.png", "image/png", 8, true), att("shot", "shot.png", "image/png", 1000), att("brief", "brief.pdf", "application/pdf", 10)] });
  const m2 = msg("m2", { attachments: [att("photo", "photo.jpg", "image/jpeg", 2000)] });
  const items = buildItems([
    { message: m1, body: [{ index: 0, src: PNG, width: 1600, height: 900, alt: "Key art" }, { index: 2, src: "https://cdn.example/banner.png", width: 1200, height: 280, alt: "" }] },
    { message: m2, body: [] },
    { message: m1, body: [] }, // a duplicate group is ignored
  ]);
  assert.deepEqual(
    items.map((i) => [i.kind, i.name, i.key]),
    [
      ["body", "hero.png", "body:acc/m1/0"],
      ["body", "banner.png", "body:acc/m1/2"],
      ["attachment", "shot.png", "att:acc/m1/shot"],
      ["attachment", "photo.jpg", "att:acc/m2/photo"],
    ],
  );
  const [hero, banner] = items;
  assert.ok(hero.kind === "body" && !hero.remote && hero.size === 8 && hero.attachment?.id === "hero");
  assert.ok(banner.kind === "body" && banner.remote && banner.size === null && banner.attachment === null);
});

// --- Save all images ---
const bodyPic = (index: number, src: string) => ({ index, src, width: 800, height: 400, alt: "" });

test("Save all images shows from two pictures up: loaded body pictures plus image attachments", () => {
  const none = msg("m1", { attachments: [att("brief", "brief.pdf", "application/pdf", 10)] });
  assert.equal(canSaveAll(messageItems(none, [])), false, "0 pictures");
  assert.equal(canSaveAll(messageItems(none, [bodyPic(0, PNG)])), false, "1 body picture");
  const oneAtt = msg("m1", { attachments: [att("shot", "shot.png", "image/png", 1000)] });
  assert.equal(canSaveAll(messageItems(oneAtt, [])), false, "1 attachment");
  assert.equal(canSaveAll(messageItems(oneAtt, [bodyPic(0, "https://cdn.example/a.png")])), true, "1 + 1");
  assert.equal(canSaveAll(messageItems(none, [bodyPic(0, PNG), bodyPic(1, "https://cdn.example/a.png")])), true, "2 body pictures");
  // Inline parts aren't counted as attachments, and a HEIC isn't viewable.
  const inlineOnly = msg("m1", { attachments: [att("hero", "hero.png", "image/png", 8, true), att("heic", "IMG.HEIC", "image/heic", 9)] });
  assert.equal(canSaveAll(messageItems(inlineOnly, [])), false);
});

test("Save all images: a picture shown in the body and also listed as an attachment counts once", () => {
  // Marked as a plain attachment but it carries a Content-ID the body embeds (8 bytes = PNG above).
  const m = msg("m1", { attachments: [{ id: "hero", filename: "hero.png", mimeType: "image/png", size: 8, contentId: "hero@x.example", inline: false }, att("shot", "shot.png", "image/png", 1000)] });
  const items = messageItems(m, [bodyPic(0, PNG), bodyPic(1, "https://cdn.example/banner.png")]);
  assert.deepEqual(items.map((i) => i.key), ["body:acc/m1/0", "body:acc/m1/1", "att:acc/m1/shot"]);
  assert.deepEqual(saveAllItems(items), [
    { kind: "attachment", attachmentId: "hero" },
    { kind: "body", src: "https://cdn.example/banner.png", name: "banner.png" },
    { kind: "attachment", attachmentId: "shot" },
  ]);
  // The same inline part twice in the body (two <img> of one cid:) is saved once.
  assert.deepEqual(
    saveAllItems(messageItems(m, [bodyPic(0, PNG), bodyPic(3, PNG)])).filter((x) => x.kind === "attachment" && x.attachmentId === "hero").length,
    1,
  );
});

test("Save all images counts only pictures that loaded in the message's own frame", () => {
  _resetFrames();
  const m = msg("m1", { attachments: [att("shot", "shot.png", "image/png", 1000)] });
  const doc = { name: "srcdoc" };
  const images = [fakeImg(doc, PNG), fakeImg(doc, "https://cdn.example/pending.png", false)];
  assert.deepEqual(frameBodyImages({ images, doc }).map((b) => b.index), [0]);
  const other = fakeFrame([], msg("m2")); // another message's pictures never count
  other.images.push(fakeImg(other.doc, PNG), fakeImg(other.doc, PNG));
  const f = fakeFrame([], m);
  f.images.push(fakeImg(f.doc, PNG), fakeImg(f.doc, "https://cdn.example/pending.png", false));
  assert.deepEqual(onScreenMessageItems(m).map((i) => i.key), ["body:acc/m1/0", "att:acc/m1/shot"]);
  f.off();
  assert.deepEqual(onScreenMessageItems(m).map((i) => i.key), ["att:acc/m1/shot"], "body closed: attachments only");
  other.off();
});
