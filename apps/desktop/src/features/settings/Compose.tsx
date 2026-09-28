// Settings → Compose: the composer font and size, with a live preview.
// OWNER: floe (font); compose settings (snippets, undo send) are ui-search's
// and mount below the font rows. Signatures have their own section.
import { useEffect, type CSSProperties } from "react";
import { updateSettings, useSettings } from "../../lib/settings";
import { asCommandError } from "../../lib/api";
import { COMPOSE_FONTS, COMPOSE_FONT_SIZE, clampFontSize, composeFontStack, loadComposeFont } from "../../lib/composeFonts";
import type { SettingsPatch } from "../../lib/types";
import { toast } from "../../components/Toast";
import { Section, Switch } from "./parts";
import "./compose-fonts.css";
import { InstantRepliesSettings, SendLaterHourSetting, SnippetsSettings, UndoSendSetting } from "../compose/settings";

const PREVIEW = "Hi Dana, Thursday at 10 works for me. I'll bring the revised draft and we can walk through it together.";

function save(patch: SettingsPatch) {
  updateSettings(patch).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

export function ComposeSection() {
  const s = useSettings();
  const size = clampFontSize(s.composeFontSize);
  // Settings is open: fetch the picker's fonts now (a few small woff2 files).
  useEffect(() => {
    COMPOSE_FONTS.forEach((f) => void loadComposeFont(f.id));
  }, []);
  return (
    <Section id="compose" icon="compose" title="Compose">
      <div className="setting-row setting-tall cf-row">
        <div>
          <span className="setting-label">Writing font</span>
          <p className="st-muted">Used in the composer only. The app keeps its own font.</p>
          <div className="cf-grid" role="radiogroup" aria-label="Writing font">
            {COMPOSE_FONTS.map((f) => (
              <button
                key={f.id}
                role="radio"
                aria-checked={s.composeFont === f.id}
                className={"cf-font" + (s.composeFont === f.id ? " selected" : "")}
                title={f.note}
                onClick={() => save({ composeFont: f.id })}
              >
                <span className="cf-aa" style={{ fontFamily: composeFontStack(f.id) }}>
                  Aa
                </span>
                <span className="cf-name">{f.name}</span>
                {f.id === "inter" && <span className="cf-default">Default</span>}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="setting-row">
        <span className="setting-label">Size</span>
        <label className="cf-size">
          <input
            type="range"
            min={COMPOSE_FONT_SIZE.min}
            max={COMPOSE_FONT_SIZE.max}
            step={1}
            value={size}
            aria-label="Writing font size"
            onChange={(e) => save({ composeFontSize: clampFontSize(Number(e.target.value)) })}
          />
          <span className="cf-size-val tnum">{size}px</span>
        </label>
      </div>
      <div className="setting-row setting-tall cf-row">
        <div className="cf-preview-wrap">
          <span className="setting-label">Preview</span>
          <p
            className="cf-preview"
            style={{ fontFamily: composeFontStack(s.composeFont), fontSize: `${size}px` } as CSSProperties}
          >
            {PREVIEW}
          </p>
          <p className="st-muted">
            Most recipients see their own default font.
          </p>
        </div>
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Reply from the account it came to</span>
          <p className="st-muted">
            Replies and forwards go only from the account the mail arrived in.
          </p>
        </div>
        <Switch label="Reply from the account it came to" on={s.lockReplyAccount} onChange={(v) => save({ lockReplyAccount: v })} />
      </div>
      <UndoSendSetting />
      <SendLaterHourSetting />
      <InstantRepliesSettings />
      <SnippetsSettings />
    </Section>
  );
}
