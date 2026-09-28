// First-run setup gate (src/app/setupGate.ts): Gmail users without a Google
// client still see setup; IMAP/Microsoft-only users don't.
import { test } from "node:test";
import assert from "node:assert/strict";
import { needsSetup } from "../src/app/setupGate.ts";

const gmail = { provider: "gmail" as const };
const imap = { provider: "imap" as const };
const microsoft = { provider: "microsoft" as const };

test("no accounts always means setup", () => {
  assert.equal(needsSetup({ configured: true }, []), true);
  assert.equal(needsSetup({ configured: false }, []), true);
});

test("Gmail accounts need their Google client, as before", () => {
  assert.equal(needsSetup({ configured: true }, [gmail]), false);
  assert.equal(needsSetup({ configured: false }, [gmail]), true);
  assert.equal(needsSetup({ configured: false }, [imap, gmail]), true);
});

test("IMAP and Microsoft accounts never need a Google client", () => {
  assert.equal(needsSetup({ configured: false }, [imap]), false);
  assert.equal(needsSetup({ configured: false }, [imap, microsoft]), false);
  assert.equal(needsSetup({ configured: true }, [microsoft]), false);
});
