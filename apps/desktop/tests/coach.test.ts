// Shortcut coach policy (lib/coach.ts): rate limits, learned keys, storage, placement.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  COACH,
  emptyCoach,
  learnedKeys,
  notifyMenuChoice,
  onMenuChoice,
  parseCoach,
  placeHint,
  recordHint,
  recordKeyUse,
  shouldHint,
} from "../src/lib/coach.ts";

const T = 1_800_000_000_000;

test("a first mouse action is hinted; the next one within the global gap is not", () => {
  let s = emptyCoach();
  assert.equal(shouldHint(s, "e", T), true);
  s = recordHint(s, "e", T);
  assert.equal(shouldHint(s, "s", T + COACH.GLOBAL_GAP_MS - 1), false);
  assert.equal(shouldHint(s, "s", T + COACH.GLOBAL_GAP_MS), true);
});

test("the same key waits KEY_GAP_MS and stops after MAX_HINTS", () => {
  let s = emptyCoach();
  let now = T;
  for (let i = 0; i < COACH.MAX_HINTS; i++) {
    assert.equal(shouldHint(s, "e", now), true, `hint ${i + 1}`);
    s = recordHint(s, "e", now);
    assert.equal(shouldHint(s, "e", now + COACH.GLOBAL_GAP_MS), false, "same key too soon");
    now += COACH.KEY_GAP_MS;
  }
  assert.equal(shouldHint(s, "e", now + 10 * COACH.KEY_GAP_MS), false, "never after the cap");
  assert.equal(shouldHint(s, "#", now), true, "other keys unaffected");
});

test("keys you press stop being hinted", () => {
  let s = emptyCoach();
  s = recordKeyUse(s, "e", T);
  // Just used it: the mouse this time was a choice, not ignorance.
  assert.equal(shouldHint(s, "e", T + 1000), false);
  assert.equal(shouldHint(s, "e", T + COACH.RECENT_USE_MS), true);
  s = recordKeyUse(s, "e", T + 5000);
  assert.equal(s.used.e, COACH.LEARNED_AFTER);
  assert.equal(shouldHint(s, "e", T + 100 * COACH.KEY_GAP_MS), false);
  assert.deepEqual(learnedKeys(s), ["e"]);
  assert.equal(shouldHint(s, "", T), false);
});

test("stored state survives a round trip and garbage starts over", () => {
  const s = recordKeyUse(recordHint(emptyCoach(), "g i", T), "e", T);
  assert.deepEqual(parseCoach(JSON.stringify(s)), s);
  assert.deepEqual(parseCoach(null), emptyCoach());
  assert.deepEqual(parseCoach("{nope"), emptyCoach());
  assert.deepEqual(parseCoach('{"used":[1],"shown":"x","lastHint":"soon"}'), emptyCoach());
});

test("the hint sits above the button, below it at the top edge, inside the window", () => {
  const view = { width: 1200, height: 800 };
  const size = { width: 120, height: 28 };
  assert.deepEqual(placeHint({ left: 500, top: 400, width: 80, height: 28 }, size, view), { left: 480, top: 364 });
  assert.deepEqual(placeHint({ left: 500, top: 10, width: 80, height: 28 }, size, view), { left: 480, top: 46 });
  assert.deepEqual(placeHint({ left: 1190, top: 400, width: 10, height: 20 }, size, view), { left: 1072, top: 364 });
  assert.deepEqual(placeHint({ left: 0, top: 400, width: 0, height: 0 }, size, view), { left: 8, top: 364 });
});

test("menu choices reach the coach only with a key", () => {
  const seen: string[] = [];
  const off = onMenuChoice((k, l) => seen.push(`${k}:${l}`));
  notifyMenuChoice("e", "Archive");
  notifyMenuChoice(undefined, "Open in Gmail");
  off();
  notifyMenuChoice("s", "Star");
  assert.deepEqual(seen, ["e:Archive"]);
});
