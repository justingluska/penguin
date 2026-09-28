// Settings → General → Appearance: the dark mode shade, sidebar theme
// swatches, "Match accent to theme" and the experimental "Dark email
// bodies". OWNER: themes. Each shade swatch is a tiny dark window that
// carries its own data-theme="dark" data-dark-shade, so it shows the shade
// from the real tokens (styles/penguin.css 1c) even in light mode. Each swatch is a tiny sidebar | list preview that
// carries its own data-sidebar-theme, so it renders from the same tokens as
// the real sidebar (styles/themes.css) in the current light/dark mode.
import { useSettings, updateSettings } from "../../lib/settings";
import type React from "react";
import { ACCENT_COLORS, CORNER_STYLES, DARK_SHADES, SIDEBAR_THEMES } from "../../lib/themes";
import type { SettingsPatch } from "../../lib/types";
import { asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import { Choice, Switch } from "./parts";

function save(patch: SettingsPatch) {
  updateSettings(patch).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

export function AppearanceSettings() {
  const s = useSettings();
  return (
    <>
      <div className="setting-row setting-tall st-appearance">
        <div className="grow min0">
          <span className="setting-label">Dark mode shade</span>
          <p className="st-muted">The background and panels in dark mode, whether it's on by choice or follows the Mac. Light mode stays as it is.</p>
          <div className="sh-grid" role="radiogroup" aria-label="Dark mode shade">
            {DARK_SHADES.map((d) => {
              const on = (s.darkShade ?? "black") === d.id;
              return (
                <button
                  key={d.id}
                  role="radio"
                  aria-checked={on}
                  className={"th-swatch" + (on ? " selected" : "")}
                  onClick={() => save({ darkShade: d.id })}
                >
                  <span className="sh-preview" data-theme="dark" data-dark-shade={d.id} aria-hidden="true">
                    <span className="sh-side">
                      <i />
                      <i className="on" />
                      <i />
                    </span>
                    <span className="sh-list">
                      <i className="unread" />
                      <i className="sel" />
                      <i />
                    </span>
                    <span className="sh-pane">
                      <b />
                      <i />
                      <i />
                    </span>
                  </span>
                  <span className="th-name">{d.name}</span>
                </button>
              );
            })}
          </div>
        </div>
      </div>
      <div className="setting-row setting-tall st-appearance">
        <div className="grow min0">
          <span className="setting-label">Sidebar theme</span>
          <p className="st-muted">Tints the sidebar in both light and dark mode.</p>
          <div className="th-grid" role="radiogroup" aria-label="Sidebar theme">
            {SIDEBAR_THEMES.map((t) => {
              const on = s.sidebarTheme === t.id;
              return (
                <button
                  key={t.id}
                  role="radio"
                  aria-checked={on}
                  data-sidebar-theme={t.id}
                  className={"th-swatch" + (on ? " selected" : "")}
                  onClick={() => save({ sidebarTheme: t.id })}
                >
                  <span className="th-preview" aria-hidden="true">
                    <span className="th-side">
                      <i />
                      <i className="on" />
                      <i />
                      <i />
                    </span>
                    <span className="th-list">
                      <i className="unread" />
                      <i />
                      <i className="unread" />
                      <i />
                    </span>
                  </span>
                  <span className="th-name">
                    <span className="th-dot" />
                    {t.name}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Sidebar text size</span>
          <p className="st-muted">Mailboxes, accounts and labels. The rest of the app stays as it is.</p>
        </div>
        <Choice
          label="Sidebar text size"
          value={String(s.sidebarTextSize ?? 0)}
          options={[
            { value: "-2", label: "Smallest" },
            { value: "-1", label: "Smaller" },
            { value: "0", label: "Default" },
            { value: "1", label: "Larger" },
            { value: "2", label: "Largest" },
          ]}
          onChange={(v) => save({ sidebarTextSize: Number(v) })}
        />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Accent color</span>
          <p className="st-muted">
            {s.matchAccent
              ? "Match accent to theme is on, so the sidebar theme's color is used."
              : "The caret, focus ring, unread dot, checkboxes and selected rows."}
          </p>
        </div>
        <div className={"ac-row" + (s.matchAccent ? " is-overridden" : "")} role="radiogroup" aria-label="Accent color">
          {ACCENT_COLORS.map((c) => {
            const on = (s.accentColor ?? "blue") === c.id;
            return (
              <button
                key={c.id}
                role="radio"
                aria-checked={on}
                aria-label={c.name}
                title={c.name}
                className={"ac-dot" + (on ? " selected" : "")}
                style={{ "--ac": c.swatch } as React.CSSProperties}
                onClick={() => save({ accentColor: c.id })}
              />
            );
          })}
        </div>
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Match accent to theme</span>
          <p className="st-muted">The caret, focus ring and unread marker take the sidebar theme's color instead of the accent color.</p>
        </div>
        <Switch label="Match accent to theme" on={s.matchAccent} onChange={(matchAccent) => save({ matchAccent })} />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Corners</span>
          <p className="st-muted">How round panels, buttons, menus and list rows are.</p>
        </div>
        <div className="setting-choice cn-choice" role="radiogroup" aria-label="Corners">
          {CORNER_STYLES.map((c) => {
            const on = (s.corners ?? "rounded") === c.id;
            return (
              <button key={c.id} role="radio" aria-checked={on} className={on ? "selected" : ""} onClick={() => save({ corners: c.id })}>
                <span className={"cn-glyph cn-" + c.id} aria-hidden="true" />
                {c.name}
              </button>
            );
          })}
        </div>
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label row-flex">
            Dark email bodies
            <span className="badge t-violet">Experimental</span>
          </span>
          <p className="st-muted">In the dark theme, HTML mail uses the sender's dark design or adapted colors. Images stay as sent.</p>
        </div>
        <Switch label="Dark email bodies" on={s.darkEmailBodies} onChange={(darkEmailBodies) => save({ darkEmailBodies })} />
      </div>
    </>
  );
}
