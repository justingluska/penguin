// OWNER: menus agent.
//
// Mock text services behind the app's context menus (app/textMenu.ts):
// Paste as a no-op (a browser can't paste from script), a tiny fixed
// "dictionary" so the spelling suggestions can be exercised in dev, Look Up
// as a console note, and label edits (sidebar label menu).
import { mockBackend, type MockHandler } from "./index";
import { deleteMockLabel, updateMockLabel } from "./mail";
import { LABEL_COLORS } from "../labelColors";

// A few deliberate misspellings and their fixes (fictional-content safe).
const GUESSES: Record<string, string[]> = {
  teh: ["the", "tech", "ten"],
  recieve: ["receive"],
  seperate: ["separate"],
  tommorow: ["tomorrow"],
  definately: ["definitely", "defiantly"],
  occured: ["occurred"],
};
const learned = new Set<string>();

export const menuHandlers: Record<string, MockHandler> = {
  copy_text: ({ text }) => {
    console.info(`[mock] copied ${String(text).length} characters`);
  },
  // A browser can't do a trusted paste from script; ⌘V still works there.
  native_paste: () => {
    console.info("[mock] menu Paste: use ⌘V in the browser build");
    return false;
  },
  spell_check: ({ word }) => {
    const w = String(word).toLowerCase();
    const guesses = GUESSES[w];
    return { misspelled: !!guesses && !learned.has(w), guesses: guesses ?? [] };
  },
  learn_spelling: ({ word }) => {
    learned.add(String(word).toLowerCase());
  },
  look_up: ({ text }) => {
    console.info(`[mock] Look Up “${text}” would open Dictionary.app`);
  },
  // Label management (sidebar label menu). Mirrors src-tauri update_label /
  // delete_label: user labels only, validated patch, then mail-changed.
  update_label: ({ accountId, labelId, patch }) => {
    const p = { ...(patch ?? {}) } as { name?: string; color?: string | null; hidden?: boolean };
    if (p.name !== undefined) {
      const name = String(p.name).trim();
      if (!name) throw { code: "invalidInput", message: "A label needs a name." };
      if ([...name].length > 225) throw { code: "invalidInput", message: "Keep the label name to 225 characters." };
      if (name.split("/").some((part) => !part.trim()))
        throw { code: "invalidInput", message: 'Each level of a nested label needs a name (no empty parts around "/").' };
      p.name = name;
    }
    if (p.color != null) {
      const c = LABEL_COLORS.find((x) => x.hex.toLowerCase() === String(p.color).toLowerCase());
      if (!c) throw { code: "invalidInput", message: `"${p.color}" isn't one of the label colors.` };
      p.color = c.hex;
    }
    const label = updateMockLabel(accountId, labelId, p);
    if (!label) throw { code: "notFound", message: `unknown label ${labelId}` };
    mockBackend.emit("penguin://mail-changed", { accountId, threadIds: [] });
    return label;
  },
  delete_label: ({ accountId, labelId }) => {
    const threadIds = deleteMockLabel(accountId, labelId);
    if (!threadIds) throw { code: "notFound", message: `unknown label ${labelId}` };
    mockBackend.emit("penguin://mail-changed", { accountId, threadIds });
  },
};
