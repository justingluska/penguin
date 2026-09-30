// What the toast says when an agent organized mail (penguin://agent-organized,
// src-tauri/src/agent/organize.rs). Pure, so it runs in node tests.
// OWNER: settings agent.

/** Tool → [verb, what follows the count]. */
const PHRASES: Record<string, [string, string]> = {
  archive: ["archived", ""],
  unarchive: ["moved", " to the inbox"],
  mark_read: ["marked", " read"],
  mark_unread: ["marked", " unread"],
  star: ["starred", ""],
  unstar: ["unstarred", ""],
  add_label: ["labelled", ""],
  remove_label: ["took a label off", ""],
  snooze: ["snoozed", ""],
  unsnooze: ["unsnoozed", ""],
  reply_later: ["moved", " to Reply Later"],
  clear_reply_later: ["took", " out of Reply Later"],
  trash: ["moved", " to Trash"],
  untrash: ["restored", " from Trash"],
  report_spam: ["reported", " as spam"],
  not_spam: ["moved", " out of Spam"],
};

/** "An agent archived 3 conversations", "An agent moved 1 conversation to Trash". */
export function agentOrganizedMessage(e: { tool: string; count: number }): string {
  const n = `${e.count} ${e.count === 1 ? "conversation" : "conversations"}`;
  const [verb, after] = PHRASES[e.tool] ?? ["changed", ""];
  return `An agent ${verb} ${n}${after}`;
}

/** Where it came from, for the toast's second line. */
export function agentOrganizedDetail(e: { tool: string; via: string }): string {
  const who = e.via === "cli" ? "From penguin-cli" : "From an MCP agent";
  return e.tool === "trash" || e.tool === "report_spam" ? `${who}. Nothing is deleted: Undo puts it back.` : `${who}. Undo puts it back.`;
}
