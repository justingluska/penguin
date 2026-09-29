// Spell checking in the app: which word a right-click asks macOS's spell
// checker about (app/spellWord.ts), and which composer fields are checked.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MAX_SPELL_WORD, spellWord, wordSpan } from "../src/app/spellWord.ts";

test("the word WebKit selected on a right-click, trimmed", () => {
  assert.equal(spellWord("recieve"), "recieve");
  // Smart selection takes the trailing space along.
  assert.equal(spellWord("recieve "), "recieve");
  assert.equal(spellWord("'tommorow'"), "tommorow");
  assert.equal(spellWord("don't"), "don't");
  assert.equal(spellWord("café"), "café");
  assert.equal(spellWord("well-known"), "well-known");
});

test("anything that isn't one word isn't checked", () => {
  for (const s of ["", " ", "a", "two words", "floe.example", "sam@harbor.example", "2026", "--", "x".repeat(MAX_SPELL_WORD + 1)]) {
    assert.equal(spellWord(s), null, JSON.stringify(s));
  }
});

test("the word's place in an input, for replacing it with a suggestion", () => {
  const v = "Plan for tommorow meeting";
  const a = v.indexOf("tommorow");
  assert.deepEqual(wordSpan(v, a, a + 8, "tommorow"), [a, a + 8]);
  // Selection with the trailing space.
  assert.deepEqual(wordSpan(v, a, a + 9, "tommorow"), [a, a + 8]);
  // Not inside the selection: nothing to replace.
  assert.equal(wordSpan(v, 0, 4, "tommorow"), null);
});

test("the subject and body are spell checked; the address fields never are", () => {
  const compose = readFileSync(new URL("../src/features/compose/index.tsx", import.meta.url), "utf8");
  const subject = compose.slice(compose.indexOf("ref={subjectRef}"), compose.indexOf("/>", compose.indexOf("ref={subjectRef}")));
  assert.match(subject, /\bspellCheck\b(?!=\{false\})/);
  const body = readFileSync(new URL("../src/features/compose/editor/RichBody.tsx", import.meta.url), "utf8");
  assert.match(body, /spellcheck: "true"/);
  const chip = readFileSync(new URL("../src/features/compose/RecipientChip.tsx", import.meta.url), "utf8");
  assert.match(chip, /spellCheck=\{false\}/);
  const recipients = compose.slice(compose.indexOf("function RecipientField"));
  assert.match(recipients, /spellCheck=\{false\}/);
});
