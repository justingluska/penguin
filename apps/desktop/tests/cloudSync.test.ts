// Settings → Accounts → Sync with iCloud (src/features/settings/cloudModel.ts):
// the line under the switch for each state the Mac can be in.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cloudLine } from "../src/features/settings/cloudModel.ts";
import type { CloudStatus } from "../src/lib/types.ts";

const NOW = 1_760_000_000_000;
const on: CloudStatus = { available: true, enabled: true, active: true, account: "available", lastSyncAt: NOW - 5 * 60_000, pending: 0 };

test("working: when it last synced and what follows you", () => {
  const l = cloudLine(on, NOW);
  assert.equal(l.warn, false);
  assert.match(l.text, /^Last synced 5 min ago\. Snoozes/);
  assert.match(cloudLine({ ...on, pending: 2 }, NOW).text, /5 min ago · 2 changes waiting/);
  assert.match(cloudLine({ ...on, lastSyncAt: undefined }, NOW).text, /^Not synced yet/);
});

test("a sync that just finished says just now, never a time in the future", () => {
  for (const at of [NOW, NOW + 800, NOW + 3_000, NOW - 4_000]) {
    assert.match(cloudLine({ ...on, lastSyncAt: at }, NOW).text, /^Last synced just now\./);
  }
});

test("a build that can't sync says why, not a warning", () => {
  const l = cloudLine({ ...on, available: false, active: false, unavailableReason: "Sync with iCloud needs macOS 14 or later." }, NOW);
  assert.deepEqual(l, { text: "Sync with iCloud needs macOS 14 or later.", warn: false });
});

test("off, signed out, restricted", () => {
  assert.match(cloudLine({ ...on, enabled: false, active: false, account: "unknown" }, NOW).text, /^Off/);
  const out = cloudLine({ ...on, active: false, account: "noAccount" }, NOW);
  assert.equal(out.warn, true);
  assert.match(out.text, /Sign in to iCloud/);
  assert.match(cloudLine({ ...on, active: false, account: "restricted" }, NOW).text, /restricted/);
  assert.match(cloudLine({ ...on, account: "unknown" }, NOW).text, /Checking iCloud/);
});

test("problems: full storage, not set up, data deleted", () => {
  const full = cloudLine({ ...on, pending: 1, problem: { kind: "quotaExceeded" } }, NOW);
  assert.equal(full.warn, true);
  assert.match(full.text, /storage is full.* · 1 change waiting/);
  assert.match(cloudLine({ ...on, problem: { kind: "notSetUp", detail: "badContainer" } }, NOW).text, /isn't ready for Penguin yet \(badContainer\)/);
  const deleted = cloudLine({ ...on, enabled: false, active: false, problem: { kind: "dataDeleted" } }, NOW);
  assert.match(deleted.text, /deleted from iCloud/);
});
