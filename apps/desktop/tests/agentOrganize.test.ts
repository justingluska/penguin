// The toast after an agent organized mail (penguin://agent-organized): what
// it says for each organizing tool, and where it came from.
import { test } from "node:test";
import assert from "node:assert/strict";
import { agentOrganizedDetail, agentOrganizedMessage } from "../src/features/settings/agentOrganized.ts";

const TOOLS = [
  "archive",
  "unarchive",
  "mark_read",
  "mark_unread",
  "star",
  "unstar",
  "add_label",
  "remove_label",
  "snooze",
  "unsnooze",
  "reply_later",
  "clear_reply_later",
  "trash",
  "untrash",
  "report_spam",
  "not_spam",
];

test("each organizing tool reads as a sentence with the count", () => {
  assert.equal(agentOrganizedMessage({ tool: "archive", count: 3 }), "An agent archived 3 conversations");
  assert.equal(agentOrganizedMessage({ tool: "trash", count: 1 }), "An agent moved 1 conversation to Trash");
  assert.equal(agentOrganizedMessage({ tool: "mark_read", count: 2 }), "An agent marked 2 conversations read");
  assert.equal(agentOrganizedMessage({ tool: "report_spam", count: 4 }), "An agent reported 4 conversations as spam");
  for (const tool of TOOLS) {
    const text = agentOrganizedMessage({ tool, count: 2 });
    assert.match(text, /^An agent .*2 conversations/, tool);
    assert.doesNotMatch(text, /changed/, `${tool} has its own words`);
  }
  // A tool this build doesn't know still reads.
  assert.equal(agentOrganizedMessage({ tool: "future_tool", count: 1 }), "An agent changed 1 conversation");
});

test("the second line names the client, and Trash and Spam say nothing is deleted", () => {
  assert.equal(agentOrganizedDetail({ tool: "archive", via: "cli" }), "From penguin-cli. Undo puts it back.");
  assert.match(agentOrganizedDetail({ tool: "trash", via: "mcp" }), /^From an MCP agent\. Nothing is deleted/);
  assert.match(agentOrganizedDetail({ tool: "report_spam", via: "mcp" }), /Nothing is deleted/);
});
