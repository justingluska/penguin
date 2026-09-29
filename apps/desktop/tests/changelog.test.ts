import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
// @ts-expect-error: a plain .mjs build script without type declarations
import { loadOverrides, overrideFor, userLine } from "../scripts/changelog.mjs";

test("internal scopes are hidden and a scope prefix is dropped", () => {
  assert.equal(userLine("docs: neutral voice"), null);
  assert.equal(userLine("release: a signed disk image"), null);
  assert.equal(userLine("publish-public: refuse shared history"), null);
  assert.equal(userLine("whats-new: plain words for 0.1.38"), null);
  assert.equal(userLine("compose: lock replies to the account"), "Lock replies to the account");
  assert.equal(userLine("Release builds: thin LTO"), "Release builds: thin LTO");
});

test("whats-new.json replaces or hides a commit's line by hash prefix", () => {
  const o = loadOverrides(join(import.meta.dirname, "../whats-new.json"));
  assert.ok(o.length > 0);
  assert.equal(overrideFor("e046839aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", o), null);
  assert.match(overrideFor("bc63c02366f182681fd276f4e37f8e5db58729e8", o), /first public release/);
  assert.equal(overrideFor("0000000000000000000000000000000000000000", o), undefined);
});
