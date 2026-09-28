// Auto labels and auto archive, as ready-made rules (Settings → Rules →
// Auto labels). Each opens the rule editor filled in, in test mode like every
// new rule, so you see what it would catch before it acts. The conditions
// are plain searches: people you have written to (is:known-sender) are never
// labelled or archived away, the way a thoughtful assistant wouldn't.
import type { RuleInput } from "../../lib/types";

export interface AutoLabel {
  key: string;
  title: string;
  /** What it does, in a line. */
  hint: string;
  draft: Partial<RuleInput>;
}

/** Leave people you've written to alone. */
const NOT_PEOPLE = "-is:known-sender";

export const AUTO_LABELS: AutoLabel[] = [
  {
    key: "marketing",
    title: "Marketing",
    hint: "Promotions → label Marketing, skip the inbox",
    draft: { name: "Auto label: Marketing", condition: `category:promotions ${NOT_PEOPLE}`, actions: [{ kind: "addLabel", label: "Marketing" }, { kind: "archive" }] },
  },
  {
    key: "news",
    title: "News",
    hint: "Newsletters → label News",
    draft: { name: "Auto label: News", condition: `is:newsletter -category:promotions ${NOT_PEOPLE}`, actions: [{ kind: "addLabel", label: "News" }] },
  },
  {
    key: "social",
    title: "Social",
    hint: "Social networks → label Social, skip the inbox",
    draft: { name: "Auto label: Social", condition: `category:social ${NOT_PEOPLE}`, actions: [{ kind: "addLabel", label: "Social" }, { kind: "archive" }] },
  },
  {
    key: "pitch",
    title: "Cold pitches",
    hint: "First mail from a stranger, not a reply or a list → label Pitch",
    draft: { name: "Auto label: Pitch", condition: `is:new-sender -is:reply -is:newsletter -has:invite ${NOT_PEOPLE}`, actions: [{ kind: "addLabel", label: "Pitch" }] },
  },
  {
    key: "notifications",
    title: "Notifications",
    hint: "No-reply and notification senders → label Notifications, skip the inbox",
    draft: {
      name: "Auto label: Notifications",
      condition: `(from:noreply OR from:no-reply OR from:notifications OR from:notification) ${NOT_PEOPLE}`,
      actions: [{ kind: "addLabel", label: "Notifications" }, { kind: "archive" }],
    },
  },
  {
    key: "receipts",
    title: "Receipts",
    hint: "Receipts, invoices and order confirmations → label Receipts",
    draft: { name: "Auto label: Receipts", condition: 'subject:receipt OR subject:invoice OR subject:"order confirmation"', actions: [{ kind: "addLabel", label: "Receipts" }] },
  },
];
