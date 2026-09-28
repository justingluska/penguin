// Right-click menus for search results (threads, attachments, people, and
// recent/saved searches). OWNER: menus agent. The panel passes each result
// and its own `run`/`edit`, so "Open" does exactly what a click does.
import type { AttachmentHit, PersonHit, SearchHit } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { isMac } from "../../lib/keyboard";
import { copyText } from "../../lib/clipboard";
import { toast } from "../../components/Toast";
import type { MenuEntries } from "../../components/ContextMenu";
import { accountById } from "../../app/store";
import { gmailThreadUrl } from "../../app/accountActions";
import { personMenu } from "../people/personMenu";
import { removeRecent, toggleSaved } from "./storage";
import { currentSettings } from "../../lib/settings";
import { isPinned } from "../smart/catalog";
import { pinSearchToSidebar, unpinSearch } from "../smart/actions";

/** The fields of a search result the menu looks at (search/index.tsx `Item`). */
export interface SearchMenuItem {
  group: string;
  hit?: SearchHit;
  att?: AttachmentHit;
  person?: PersonHit;
  query?: string;
}

export interface SearchMenuCtx {
  /** Open like a click; `keep` = ⌘-click (keep the search open). */
  open: (keep: boolean) => void;
  /** Replace the query. */
  edit: (q: string) => void;
  saved: string[];
}

function threadEntries(accountId: string, threadId: string, ctx: SearchMenuCtx): MenuEntries {
  // Gmail links need Gmail API ids.
  const account = accountById(accountId)?.provider === "gmail" ? accountById(accountId) : undefined;
  return [
    { label: "Open", icon: "expand", keys: "enter", onSelect: () => ctx.open(false) },
    { label: "Open, keep search", icon: "columns", keys: "mod+enter", onSelect: () => ctx.open(true) },
    { type: "separator" },
    account && { label: "Copy Gmail link", icon: "link", onSelect: () => void copyText(gmailThreadUrl(account.email, threadId), "Link copied") },
    account && { label: "Open in Gmail", icon: "external", onSelect: () => void api.openExternal(gmailThreadUrl(account.email, threadId)) },
  ];
}

export function searchItemMenu(it: SearchMenuItem, ctx: SearchMenuCtx): MenuEntries {
  if (it.hit) {
    const h = it.hit;
    return [
      ...threadEntries(h.accountId, h.threadId, ctx),
      { type: "separator" },
      { label: `Search mail from ${h.from.name?.trim() || h.from.email}`, icon: "search", onSelect: () => ctx.edit(`from:${h.from.email}`) },
      { label: "Copy sender address", icon: "at", onSelect: () => void copyText(h.from.email, "Email copied") },
    ];
  }
  if (it.att) {
    const a = it.att;
    const f = a.attachment;
    return [
      { label: "Open conversation", icon: "expand", keys: "enter", onSelect: () => ctx.open(false) },
      {
        label: isMac ? "Save to Downloads" : "Save",
        icon: "download",
        onSelect: () =>
          void api.saveAttachment(a.accountId, a.messageId, f.id).then(
            (path) => toast({ message: `Saved ${f.filename} to Downloads`, action: { label: "Open", run: () => void api.openPath(path) } }),
            (e) => toast({ tone: "error", message: `Couldn't save ${f.filename}: ${asCommandError(e).message}` }),
          ),
      },
      { type: "separator" },
      { label: "Copy file name", icon: "copy", onSelect: () => void copyText(f.filename, "File name copied") },
      { label: `Search mail from ${a.from.name?.trim() || a.from.email}`, icon: "search", onSelect: () => ctx.edit(`from:${a.from.email}`) },
    ];
  }
  if (it.person) {
    const p = it.person.address;
    return [
      { label: "Show mail from them", icon: "search", keys: "enter", onSelect: () => ctx.open(false) },
      { type: "separator" },
      ...personMenu(p),
    ];
  }
  if (it.query !== undefined && (it.group === "recent" || it.group === "saved")) {
    const q = it.query;
    const saved = ctx.saved.includes(q);
    return [
      { label: "Search", icon: "search", keys: "enter", onSelect: () => ctx.edit(q) },
      { type: "separator" },
      { label: saved ? "Remove from saved" : "Save search", icon: "pin", onSelect: () => void toggleSaved(q) },
      // A saved search can live in the sidebar as a view (features/smart).
      isPinned(currentSettings().smartViews, q)
        ? {
            label: "Remove from sidebar",
            icon: "sidebar",
            onSelect: () => {
              const c = currentSettings().smartViews.custom.find((x) => x.query === q.trim().replace(/\s+/g, " "));
              if (c) unpinSearch(c.id);
            },
          }
        : { label: "Pin to sidebar", icon: "sidebar", onSelect: () => void pinSearchToSidebar(q, false) },
      it.group === "recent" && { label: "Remove from recent", icon: "x", onSelect: () => removeRecent(q) },
      { label: "Copy query", icon: "copy", onSelect: () => void copyText(q, "Query copied") },
    ];
  }
  return [];
}
