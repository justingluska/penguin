// "Read by Alex Rivera · 3:12 PM" above a message you sent, when a read
// receipt came back (Settings → Privacy "Ask for read receipts").
import type { MessageView } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { shortDate } from "../../lib/format";
import { RECEIPT_NOTE, receiptLine } from "./receipts";
import "./receipts.css";

export function ReadReceiptLine({ m }: { m: MessageView }) {
  const line = receiptLine(m);
  if (!line) return null;
  const title = [...line.details.map((d) => `${d.who}: ${d.what}, ${new Date(d.at).toLocaleString()}`), "", RECEIPT_NOTE].join("\n");
  return (
    <div className="rr-line" title={title}>
      <Icon name="check" size="xs" />
      <span>{line.text}</span>
      <span className="rr-time tnum">· {shortDate(line.at)}</span>
    </div>
  );
}
