// Which word a right-click in a text field asks the spell checker about
// (app/textMenu.ts). Pure, so node --test loads it.
//
// On the Mac, WebKit selects the word under a right-click in an editable
// field before the contextmenu event (EventHandler::sendContextMenuEvent,
// "select on contextual menu click"), so the selection is usually exactly
// that word; with nothing selected, the word under the pointer is used.

/** Longest word the spelling commands accept (text_services.rs MAX_WORD). */
export const MAX_SPELL_WORD = 100;

const WORD = /^[\p{L}\p{M}'’-]+$/u;

/**
 * The selection as one word to check, or null when it's more (or less) than
 * one: a phrase, a number, an address. Hanging apostrophes and hyphens are
 * trimmed, and whitespace WebKit's smart selection took along is ignored.
 */
export function spellWord(selected: string): string | null {
  const w = selected.trim().replace(/^['’-]+|['’-]+$/g, "");
  if (w.length < 2 || w.length > MAX_SPELL_WORD || !WORD.test(w)) return null;
  // A letter somewhere (not just "--").
  return /\p{L}/u.test(w) ? w : null;
}

/** [start, end) of `word` in an input's value around its selection, for replacing it. */
export function wordSpan(value: string, selStart: number, selEnd: number, word: string): [number, number] | null {
  const at = value.indexOf(word, selStart);
  if (at < 0 || at + word.length > Math.max(selEnd, selStart + word.length)) return null;
  return [at, at + word.length];
}
