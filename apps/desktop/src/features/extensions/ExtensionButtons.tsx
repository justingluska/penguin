// The allowed extensions' actions as icon buttons in a message's header,
// next to Reply and Forward. Nothing shows until an extension is allowed.
import type { MessageView } from "../../lib/types";
import { ExtensionIcon } from "./ExtensionIcon";
import { actionLabel, allowedActions, runExtensionAction, useExtensions } from "./state";

export function ExtensionButtons({ m }: { m: MessageView }) {
  const actions = allowedActions(useExtensions());
  if (actions.length === 0 || m.labelIds.includes("DRAFT")) return null;
  return (
    <>
      {actions.map((a) => (
        <button
          key={`${a.ext.id}/${a.action.id}`}
          type="button"
          className="btn btn-ghost btn-sm btn-icon ext-btn"
          title={actionLabel(a)}
          aria-label={actionLabel(a)}
          onClick={(e) => {
            e.stopPropagation();
            void runExtensionAction(a, m);
          }}
        >
          <ExtensionIcon icon={a.action.icon} svg={a.action.iconSvg} />
        </button>
      ))}
    </>
  );
}
