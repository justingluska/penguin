// Mock Handoff with the iPhone (src-tauri/src/handoff.rs): what a window
// offers, and an activity "from the iPhone" waiting at launch, as when
// Handoff starts Penguin: ?mockHandoff=<accountId>/<threadId> for a
// conversation, ?mockHandoff=draft:<accountId>/<draftId> for a draft,
// ?mockHandoff=unreadable for one this Mac can't read.
import type { HandoffActivity, HandoffOpen } from "../types";
import type { MockHandler } from "./index";

/** What a window last offered (for poking at in the console). */
export let mockHandoffActivity: HandoffActivity = { kind: "none" };

function fromQuery(): HandoffOpen | null {
  const q = typeof location === "undefined" ? null : new URLSearchParams(location.search).get("mockHandoff");
  if (!q) return null;
  if (q === "unreadable") return { kind: "unreadable", activity: "conversation" };
  const draft = q.startsWith("draft:");
  const [accountId, id] = (draft ? q.slice("draft:".length) : q).split("/");
  if (!accountId || !id) return null;
  return draft ? { kind: "draft", accountId, draftId: id, threadId: null } : { kind: "conversation", accountId, threadId: id };
}

let pending: HandoffOpen | null = fromQuery();

export const handoffHandlers: Record<string, MockHandler> = {
  set_handoff_activity: ({ activity }) => {
    mockHandoffActivity = activity as HandoffActivity;
  },
  take_handoff_open: () => {
    const open = pending;
    pending = null;
    return open;
  },
};
