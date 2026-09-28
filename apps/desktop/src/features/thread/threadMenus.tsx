// Right-click menus inside an open thread: a message (its header or its
// collapsed row) and an attachment card. OWNER: menus agent.
//
// The message body itself is a sandboxed no-script iframe; WebKit doesn't
// run the app's listeners inside it, so right-clicks on the body text and
// its links still get WebKit's own menu (see app/textMenu.ts).
import type { AttachmentMeta, MessageView } from "../../lib/types";
import { api } from "../../lib/api";
import { setUi } from "../../lib/ui";
import { copyText } from "../../lib/clipboard";
import type { MenuEntries } from "../../components/ContextMenu";
import { accountById } from "../../app/store";
import { isUnread, markUnreadAndSay, toggleRead } from "../../app/actions";
import { openMessageDetails } from "./MessageDetails";
import { downloadAttachment, openInDefaultApp, revealSaved, saveAttachmentAs, savedAttachmentPath } from "./AttachmentPreview";
import { copyImage, nativeFiles, openInPreview, openLabel, revealImage, saveAllImages, saveImage, saveImageAs } from "../image-viewer/actions";
import { imageMenu } from "../image-viewer/imageMenu";
import { attachmentItem, canSaveAll, isViewableAttachment, onScreenMessageItems } from "../image-viewer/items";
import { openAttachment } from "./openAttachment";
import { currentSettings } from "../../lib/settings";
import { startUnsubscribe } from "../unsubscribe/actions";
import { COPY_CONVERSATION_KEYS, copyConversation, copyMessage } from "./copy";

function gmailMessageUrl(email: string, messageId: string): string {
  return `https://mail.google.com/mail/u/${encodeURIComponent(email)}/#all/${encodeURIComponent(messageId)}`;
}

export function messageMenu(m: MessageView, opts: { expanded: boolean; onToggle?: () => void }): MenuEntries {
  const thread = { accountId: m.accountId, threadId: m.threadId };
  const compose = (mode: "reply" | "replyAll" | "forward") => () =>
    setUi({ overlay: "compose", composeContext: { mode, thread, messageId: m.id } });
  const account = accountById(m.accountId);
  const draft = m.labelIds.includes("DRAFT");
  const pictures = onScreenMessageItems(m);
  return [
    !draft && { label: "Reply", icon: "reply", keys: "r", onSelect: compose("reply") },
    !draft && { label: "Reply all", icon: "replyall", keys: "a", onSelect: compose("replyAll") },
    !draft && { label: "Forward", icon: "forward", keys: "f", onSelect: compose("forward") },
    { type: "separator" },
    opts.onToggle && { label: opts.expanded ? "Collapse" : "Expand", icon: opts.expanded ? "minus" : "expand", onSelect: opts.onToggle },
    !draft &&
      (isUnread(thread)
        ? { label: "Mark conversation read", icon: "mail", keys: "u", onSelect: () => toggleRead([thread]) }
        : { label: "Mark conversation unread", icon: "unread", keys: "shift+u", onSelect: () => markUnreadAndSay([thread]) }),
    { type: "separator" },
    // With From/To/Date and attachment names, reply history cut; "Copy text" is the raw body.
    { label: "Copy message", icon: "copy", onSelect: () => copyMessage(m) },
    {
      label: "Copy conversation",
      icon: "copy",
      keys: COPY_CONVERSATION_KEYS,
      onSelect: () => copyConversation(thread),
    },
    {
      label: "Copy text",
      icon: "copy",
      disabled: m.bodyPending ? "Still downloading this message" : !m.bodyText.trim() ? "No plain text in this message" : false,
      onSelect: () => void copyText(m.bodyText, "Message text copied"),
    },
    { label: "Copy sender address", icon: "at", onSelect: () => void copyText(m.from.email, "Email copied") },
    canSaveAll(pictures) && { label: `Save all images (${pictures.length})`, text: "Save all images", icon: "download", onSelect: () => void saveAllImages(m, pictures) },
    m.unsubscribe &&
      currentSettings().unsubscribeButton && {
        label: m.unsubscribe.unsubscribed && m.unsubscribe.unsubscribed.method !== "link" ? "Unsubscribe again" : "Unsubscribe",
        icon: "belloff",
        onSelect: () => void startUnsubscribe(m),
      },
    { type: "separator" },
    { label: "Message details", icon: "info", onSelect: () => openMessageDetails(m) },
    { label: "Show original", icon: "code", onSelect: () => openMessageDetails(m, true) },
    account?.provider === "gmail" && { label: "Open in Gmail", icon: "external", onSelect: () => void api.openExternal(gmailMessageUrl(account.email, m.id)) },
  ];
}

export function attachmentMenu(m: MessageView, a: AttachmentMeta): MenuEntries {
  const copyName: MenuEntries = [{ type: "separator" }, { label: "Copy file name", icon: "copy", onSelect: () => void copyText(a.filename, "File name copied") }];
  const saved = savedAttachmentPath(m, a);
  // Pictures get the image viewer's menu (Copy Image, Open in Preview…).
  if (!a.inline && isViewableAttachment(a)) {
    const it = attachmentItem(m, a);
    return [
      ...imageMenu(
        it,
        { where: "card", saved: saved !== null, nativeFiles, openLabel },
        {
          view: () => openAttachment(m, a),
          copy: () => void copyImage(it),
          copyAddress: () => undefined,
          save: () => void saveImage(it),
          saveAs: () => void saveImageAs(it),
          openInPreview: () => void openInPreview(it),
          reveal: () => revealImage(it),
        },
      ),
      ...copyName,
    ];
  }
  return [
    { label: "Preview", icon: "eye", onSelect: () => openAttachment(m, a) },
    { label: "Open", icon: "external", onSelect: () => void openInDefaultApp(m, a) },
    { type: "separator" },
    { label: "Save to Downloads", icon: "download", onSelect: () => void downloadAttachment(m, a) },
    nativeFiles && { label: "Save As…", text: "Save As", icon: "folder", onSelect: () => void saveAttachmentAs(m, a) },
    nativeFiles && saved && { label: "Show in Finder", icon: "folder", onSelect: () => void revealSaved(saved, a.filename) },
    ...copyName,
  ];
}
