// Clicking an attachment: pictures open in the image viewer (full window,
// zoom, ←/→ through every picture in the conversation); everything else in
// the attachment preview panel.
import type { AttachmentMeta, MessageView } from "../../lib/types";
import { openImageAttachment } from "../image-viewer/ImageViewer";
import { isViewableAttachment } from "../image-viewer/items";
import { openAttachmentPreview } from "./AttachmentPreview";

export function openAttachment(m: MessageView, a: AttachmentMeta) {
  if (!a.inline && isViewableAttachment(a)) openImageAttachment(m, a);
  else openAttachmentPreview(m, a.id);
}
