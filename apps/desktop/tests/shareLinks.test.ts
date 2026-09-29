// Share links, the UI's pure part (features/share/model.ts): what a picture
// or attachment shares, the "expires in" wording, the endpoint tidy-up, and
// the check on a pending share handed over from another window.
import { test } from "node:test";
import assert from "node:assert/strict";
import { asShareTarget, attachmentShare, expiresIn, formMissing, lifetimeLabel, pictureShare, tidyEndpoint } from "../src/features/share/model.ts";
import { attachmentItem, buildItems } from "../src/features/image-viewer/items.ts";
import type { AttachmentMeta, MessageView, ShareLinkConfigInput } from "../src/lib/types.ts";

const PNG = "data:image/png;base64,iVBORw0KGgo=";
const REMOTE = "https://cdn.floe.example/launch/hero.jpg";
const pdf: AttachmentMeta = { id: "a2", filename: "Q3 report.pdf", mimeType: "application/pdf", size: 312_000, contentId: null, inline: false };
const shot: AttachmentMeta = { id: "a1", filename: "screenshot.png", mimeType: "image/png", size: 1000, contentId: null, inline: false };
const msg = (attachments: AttachmentMeta[] = []) =>
  ({ accountId: "acc", id: "m1", threadId: "t", date: 1, subject: "Launch", from: { name: "Juniper Hale", email: "juniper@floe.example" }, attachments }) as MessageView;

test("an attachment is shared by id, with its name and size for the toasts", () => {
  assert.deepEqual(attachmentShare(msg([pdf]), pdf), {
    request: { kind: "attachment", accountId: "acc", messageId: "m1", attachmentId: "a2" },
    name: "Q3 report.pdf",
    size: 312_000,
  });
  const it = attachmentItem(msg([shot]), shot);
  assert.deepEqual(pictureShare(it).request, { kind: "attachment", accountId: "acc", messageId: "m1", attachmentId: "a1" });
});

test("a body picture is shared by the source Penguin already has", () => {
  const [embedded, remote] = buildItems([
    {
      message: msg(),
      body: [
        { index: 0, src: PNG, width: 40, height: 40, alt: "" },
        { index: 1, src: REMOTE, width: 800, height: 400, alt: "" },
      ],
    },
  ]);
  const e = pictureShare(embedded);
  assert.ok(e.request.kind === "picture" && e.request.src === PNG);
  const r = pictureShare(remote);
  assert.ok(r.request.kind === "picture" && r.request.src === REMOTE && r.request.name === r.name);
});

test("expiry in words", () => {
  const now = 1_000_000;
  assert.equal(expiresIn(now + 24 * 3_600_000, now), "expires in 24 hours");
  assert.equal(expiresIn(now + 24 * 3_600_000 - 400, now), "expires in 24 hours", "the round trip's milliseconds don't show");
  assert.equal(expiresIn(now + 3_600_000, now), "expires in 1 hour");
  assert.equal(expiresIn(now + 7 * 24 * 3_600_000, now), "expires in 7 days");
  assert.equal(expiresIn(now + 20 * 60_000, now), "expires in 20 minutes");
  assert.equal(lifetimeLabel("7d"), "7 days");
});

test("an R2 S3 API address pasted with its bucket is split into endpoint and bucket", () => {
  assert.deepEqual(tidyEndpoint(" https://abc123.r2.cloudflarestorage.com/penguin-shares ", ""), { endpoint: "https://abc123.r2.cloudflarestorage.com", bucket: "penguin-shares" });
  assert.deepEqual(tidyEndpoint("https://abc123.r2.cloudflarestorage.com/penguin-shares", "penguin-shares"), { endpoint: "https://abc123.r2.cloudflarestorage.com", bucket: "penguin-shares" });
  // A different bucket typed: leave both for the backend to explain.
  assert.deepEqual(tidyEndpoint("https://abc123.r2.cloudflarestorage.com/one", "two"), { endpoint: "https://abc123.r2.cloudflarestorage.com/one", bucket: "two" });
  assert.deepEqual(tidyEndpoint("https://abc123.r2.cloudflarestorage.com/", "b"), { endpoint: "https://abc123.r2.cloudflarestorage.com", bucket: "b" });
  assert.deepEqual(tidyEndpoint("not a url", "b"), { endpoint: "not a url", bucket: "b" });
});

test("the form needs every storage field, and a secret unless one is saved", () => {
  const f: ShareLinkConfigInput = { endpoint: "https://abc.r2.cloudflarestorage.com", bucket: "b", region: "", accessKeyId: "k", secretAccessKey: "", lifetime: "24h", deleteOnExpiry: true, allowAgents: false };
  assert.equal(formMissing(f, false), "Enter the secret access key");
  assert.equal(formMissing(f, true), null);
  assert.equal(formMissing({ ...f, endpoint: " " }, true), "Enter the endpoint");
  assert.equal(formMissing({ ...f, secretAccessKey: "s" }, false), null);
});

test("a pending share from another window is checked, never trusted", () => {
  const good = { request: { kind: "attachment", accountId: "acc", messageId: "m1", attachmentId: "a2" }, name: "Q3 report.pdf", size: 312_000 };
  assert.deepEqual(asShareTarget(good), good);
  assert.equal(asShareTarget({ ...good, size: "big" })?.size, null);
  const pic = { request: { kind: "picture", accountId: "acc", messageId: "m1", src: PNG, name: "p.png" }, name: "p.png", size: null };
  assert.deepEqual(asShareTarget(pic), pic);
  for (const bad of [
    null,
    "x",
    { name: "x" },
    { ...good, request: { ...good.request, kind: "file" } },
    { ...good, request: { ...good.request, attachmentId: "" } },
    { ...pic, request: { ...pic.request, src: "javascript:alert(1)" } },
    { ...pic, request: { ...pic.request, src: "http://tracker.example/p.gif" } },
    { ...pic, request: { ...pic.request, src: "file:///etc/passwd" } },
  ]) {
    assert.equal(asShareTarget(bad), null, JSON.stringify(bad));
  }
});
