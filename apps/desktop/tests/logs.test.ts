// The log viewer's parser (features/logs/parse.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { isProblem, parseLog } from "../src/features/logs/parse.ts";

test("parses level, target and text, skipping spans", () => {
  const [a, b] = parseLog([
    "2026-09-25T08:03:40.551290Z  WARN penguin_ui: Gmail didn't answer: timed out source=command what=save_attachment",
    "2026-09-25T08:04:00.000000Z  INFO sync{account=sam@northwind.example}: penguin_gmail::sync: done changed=4",
  ]);
  assert.equal(a.level, "WARN");
  assert.equal(a.target, "penguin_ui");
  assert.equal(a.text, "Gmail didn't answer: timed out source=command what=save_attachment");
  assert.equal(a.at, Date.parse("2026-09-25T08:03:40.551Z"));
  assert.equal(b.target, "penguin_gmail::sync");
  assert.equal(b.text, "done changed=4");
  assert.ok(isProblem(a) && !isProblem(b));
});

test("continuation lines join the entry above", () => {
  const e = parseLog([
    "stray first line",
    "2026-09-25T08:05:00.000000Z ERROR penguin_desktop_lib::logging: panic panic=boom",
    "   0: backtrace frame",
  ]);
  assert.equal(e.length, 2);
  assert.equal(e[0].level, null);
  assert.equal(e[1].text, "panic panic=boom\n   0: backtrace frame");
});
