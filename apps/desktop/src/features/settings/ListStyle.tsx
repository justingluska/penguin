// Settings → General → List style: how the conversation list's rows look
// (lib/listStyle.ts; the CSS is styles/list-styles.css). Laid out like the
// sidebar theme swatches beside it: each option is a tiny three-row preview
// of the style (unread, selected, read), then its name.
import { updateSettings, useSetting } from "../../lib/settings";
import { LIST_STYLES } from "../../lib/listStyle";
import { asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import type { ListStyle } from "../../lib/types";

function pick(listStyle: ListStyle) {
  updateSettings({ listStyle }).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

export function ListStyleRow() {
  const current = useSetting("listStyle");
  const blurb = LIST_STYLES.find((s) => s.id === current)?.blurb;
  return (
    <div className="setting-row setting-tall st-list-style">
      <div className="grow min0">
        <span className="setting-label">List style</span>
        <p className="st-muted">How conversations look in the list. {blurb}</p>
        <div className="ls-grid" role="radiogroup" aria-label="List style">
          {LIST_STYLES.map((s) => {
            const on = current === s.id;
            return (
              <button
                key={s.id}
                role="radio"
                aria-checked={on}
                title={s.blurb}
                className={"ls-swatch" + (on ? " selected" : "")}
                onClick={() => pick(s.id)}
              >
                <span className="ls-preview" data-ls={s.id} aria-hidden="true">
                  {(["unread", "sel", ""] as const).map((state, i) => (
                    <span key={i} className={"ls-row" + (state ? " " + state : "")}>
                      <b />
                      <span>
                        <i />
                        {s.id === "mail" && <i />}
                        <i />
                      </span>
                    </span>
                  ))}
                </span>
                <span className="ls-name">{s.name}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
