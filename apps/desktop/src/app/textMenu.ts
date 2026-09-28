// Global right-click policy. WebKit's own menu (Look Up, Translate, Reload,
// Inspect…) never shows in the app: components open their menus through
// useContextMenu (components/ContextMenu.tsx), and every other right-click
// lands here, where it is either an editing menu (text fields, the composer)
// or a selection menu (selected text anywhere), or nothing at all.
//
// The one place this can't reach is inside a message body: it's a
// sandboxed no-script iframe, and WebKit doesn't run the parent's listeners
// in such a frame, so right-clicks there (and on its links) still get
// WebKit's menu. See features/message-body/MessageBody.tsx.
import { api } from "../lib/api";
import { isMac } from "../lib/keyboard";
import { setUi } from "../lib/ui";
import { toast } from "../components/Toast";
import { hasTextSelectionAt, openMenu, type MenuEntries } from "../components/ContextMenu";

type Field = HTMLInputElement | HTMLTextAreaElement | HTMLElement;

const TEXT_INPUTS = new Set(["text", "search", "email", "url", "tel", "password", "number", ""]);

function editableAt(t: EventTarget | null): Field | null {
  if (!(t instanceof Element)) return null;
  if (t instanceof HTMLTextAreaElement) return t;
  if (t instanceof HTMLInputElement) return TEXT_INPUTS.has(t.type) ? t : null;
  if (t instanceof HTMLElement && t.isContentEditable) {
    // The editing host, not the paragraph inside it.
    let host: HTMLElement = t;
    while (host.parentElement?.isContentEditable) host = host.parentElement;
    return host;
  }
  return null;
}

function isInput(f: Field): f is HTMLInputElement | HTMLTextAreaElement {
  return f instanceof HTMLInputElement || f instanceof HTMLTextAreaElement;
}

function fieldSelection(f: Field): string {
  if (isInput(f)) {
    // Passwords never leave their field through this menu.
    if (f instanceof HTMLInputElement && f.type === "password") return "";
    const { selectionStart: a, selectionEnd: b } = f;
    return a !== null && b !== null && b > a ? f.value.slice(a, b) : "";
  }
  const sel = document.getSelection();
  if (!sel || sel.isCollapsed || !sel.anchorNode || !f.contains(sel.anchorNode)) return "";
  return sel.toString();
}

function readOnly(f: Field): boolean {
  return isInput(f) ? f.readOnly || f.disabled : false;
}

/** Short enough to be a word or phrase worth looking up or searching for. */
function phrase(text: string): string | null {
  const t = text.trim().replace(/\s+/g, " ");
  return t && t.length <= 60 ? t : null;
}

function quote(t: string): string {
  return `“${t.length > 28 ? t.slice(0, 27) + "…" : t}”`;
}

function exec(cmd: "cut" | "copy" | "selectAll") {
  // Still a user gesture (the menu click), which WebKit requires for cut/copy.
  if (!document.execCommand(cmd)) toast({ tone: "error", message: `Couldn't ${cmd === "selectAll" ? "select all" : cmd}` });
}

async function paste(f: Field) {
  // The field keeps focus through the menu (its mousedown is prevented), so
  // the native paste lands where ⌘V would: a trusted paste event, and the
  // composer's own paste handling (HTML sanitizing, files) runs as usual.
  f.focus({ preventScroll: true });
  try {
    if (!(await api.nativePaste())) toast({ tone: "error", message: "Couldn't paste here" });
  } catch (e) {
    toast({ tone: "error", message: "Couldn't paste", detail: String((e as { message?: string })?.message ?? e) });
  }
}

/** The word under (x, y) inside a contenteditable, as a Range. */
function wordRangeAt(x: number, y: number, host: HTMLElement): Range | null {
  const r = document.caretRangeFromPoint?.(x, y);
  const node = r?.startContainer;
  if (!r || !node || node.nodeType !== Node.TEXT_NODE || !host.contains(node)) return null;
  const text = node.textContent ?? "";
  const isWord = (c: string | undefined) => !!c && /[\p{L}\p{M}'’-]/u.test(c);
  let a = r.startOffset;
  let b = r.startOffset;
  while (a > 0 && isWord(text[a - 1])) a--;
  while (b < text.length && isWord(text[b])) b++;
  // Trim apostrophes/hyphens hanging off either end.
  while (a < b && /['’-]/.test(text[a])) a++;
  while (b > a && /['’-]/.test(text[b - 1])) b--;
  if (b - a < 2) return null;
  const w = document.createRange();
  w.setStart(node, a);
  w.setEnd(node, b);
  return w;
}

function replaceRange(range: Range, text: string) {
  const sel = document.getSelection();
  if (!sel) return;
  sel.removeAllRanges();
  sel.addRange(range);
  document.execCommand("insertText", false, text);
}

function lookUp(text: string) {
  api.lookUp(text).catch((e) => toast({ tone: "error", message: "Couldn't open Dictionary", detail: String(e?.message ?? e) }));
}

function searchFor(text: string) {
  setUi({ overlay: "search", searchPrefill: text });
}

function selectionItems(text: string, offerSearch = true): MenuEntries {
  const p = phrase(text);
  return [
    p && offerSearch && { label: `Search Penguin for ${quote(p)}`, icon: "search", onSelect: () => searchFor(p) },
    p && isMac && { label: `Look Up ${quote(p)}`, icon: "book", onSelect: () => lookUp(p) },
  ];
}

async function editMenu(e: MouseEvent, f: Field) {
  const selected = fieldSelection(f);
  const ro = readOnly(f);
  const at = { x: e.clientX, y: e.clientY };

  // Spelling (contenteditable with spellcheck on, e.g. the composer body):
  // the word under the pointer, checked by macOS's spell checker.
  let spelling: MenuEntries = [];
  if (!isInput(f) && f.spellcheck && !selected) {
    const range = wordRangeAt(e.clientX, e.clientY, f);
    const word = range?.toString();
    if (range && word) {
      try {
        const res = await api.spellCheck(word);
        if (res.misspelled) {
          spelling = [
            ...(res.guesses.length
              ? res.guesses.slice(0, 6).map((g) => ({ label: g, text: g, onSelect: () => replaceRange(range, g) }))
              : [{ label: "No guesses found", disabled: true }]),
            { type: "separator" as const },
            {
              label: "Learn spelling",
              onSelect: () => api.learnSpelling(word).catch(() => toast({ tone: "error", message: "Couldn't learn that word" })),
            },
            { type: "separator" as const },
          ];
        }
      } catch {
        // Spell check unavailable (mock/browser): the menu just has no suggestions.
      }
    }
  }

  openMenu(
    at,
    [
      ...spelling,
      { label: "Cut", icon: "scissors", keys: "mod+x", disabled: !selected || ro, onSelect: () => exec("cut") },
      { label: "Copy", icon: "copy", keys: "mod+c", disabled: !selected, onSelect: () => exec("copy") },
      { label: "Paste", icon: "clipboard", keys: "mod+v", disabled: ro, onSelect: () => void paste(f) },
      { label: "Select all", keys: "mod+a", onSelect: () => (isInput(f) ? f.select() : exec("selectAll")) },
      { type: "separator" },
      // Not from inside the search box itself: that's already a search.
      ...(selected ? selectionItems(selected, !f.closest(".sx-host")) : []),
    ],
    { label: "Edit" },
  );
}

function selectionMenu(e: MouseEvent) {
  const text = document.getSelection()?.toString() ?? "";
  openMenu(
    { x: e.clientX, y: e.clientY },
    [
      { label: "Copy", icon: "copy", keys: "mod+c", onSelect: () => exec("copy") },
      { type: "separator" },
      ...selectionItems(text),
    ],
    { label: "Selection" },
  );
}

/** Install once at app start; returns the uninstaller. */
export function installContextMenus(): () => void {
  const onContext = (e: MouseEvent) => {
    // A component's menu (useContextMenu) already claimed this one.
    if (e.defaultPrevented) return;
    e.preventDefault();
    const f = editableAt(e.target);
    if (f) void editMenu(e, f);
    else if (hasTextSelectionAt(e)) selectionMenu(e);
  };
  // Bubble phase on window: React's handlers (on the root) run first.
  window.addEventListener("contextmenu", onContext);
  return () => window.removeEventListener("contextmenu", onContext);
}
