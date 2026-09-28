// Copy a conversation (or one message) as clean plain text: the thread
// toolbar's Copy, ⇧C, the ⌘K palette, and the message and row menus. The
// text itself is built in conversationText.ts.
//
// Reads the thread the pane already has. A conversation that isn't cached
// yet (a row menu, ⇧C before the preview loaded) is read from the local
// store; get_thread never waits on the network, and a message whose body is
// still downloading is copied from its snippet.
import type { MessageView, ThreadRef } from "../../lib/types";
import { getUi } from "../../lib/ui";
import { copyText, copyTextLater } from "../../lib/clipboard";
import { cachedThread, fetchThread } from "../../app/store";
import { conversationText, messageText } from "./conversationText";

export const COPY_CONVERSATION_KEYS = "shift+c";

export function copyConversation(ref: ThreadRef | null = getUi().selected): void {
  if (!ref) return;
  const done = "Conversation copied";
  const hit = cachedThread(ref);
  if (hit) {
    void copyText(conversationText(hit), done);
    return;
  }
  const text = fetchThread(ref).then((t) => {
    if (!t) throw new Error("This conversation is no longer available.");
    return conversationText(t);
  });
  void copyTextLater(text, done);
}

export function copyMessage(m: MessageView): void {
  const thread = cachedThread({ accountId: m.accountId, threadId: m.threadId }) ?? { subject: m.subject, messages: [m] };
  // The cached thread may predate this view of the message (images loaded, body arrived).
  const messages = thread.messages.map((x) => (x.id === m.id ? m : x));
  void copyText(messageText({ subject: thread.subject, messages }, m.id), "Message copied");
}
