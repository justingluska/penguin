// The sheet an extension action runs in: its form (fields from
// extension.json: text, textarea, select, checkbox, prefilled from the
// message) and, when the program answers with `confirm`, what it asks
// before acting (docs/EXTENSIONS.md → Asking before acting). The sheet runs
// the action itself: the button shows it's running, a failure shows in the
// sheet, and confirming runs the action again with `continuation`.
//
// Keys: ⌘Return runs (Return too, outside a text area); ↑↓ pick a choice;
// Esc cancels. While a run is going the sheet stays up.
import { useRef, useState } from "react";
import type { ExtensionConfirm, ExtensionResult, MessageView } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { Modal, openModal } from "../../components/Modal";
import { Select } from "../../components/Select";
import { Icon } from "../../components/Icon";
import { Keys } from "../../components/Kbd";
import { ExtensionIcon } from "./ExtensionIcon";
import { fillTemplate, type MessageAction } from "./state";
import "./extensions.css";

/** A confirm the first run (an action without a form) already asked. */
export type SheetStart = { confirm: ExtensionConfirm; form: Record<string, string> } | null;

/**
 * Show the sheet: the action's form, or (`start`) a confirm. `done` gets
 * the program's final answer; it isn't called when the person cancels.
 */
export function openExtensionSheet(a: MessageAction, m: MessageView, start: SheetStart, done: (r: ExtensionResult) => void): void {
  openModal((close) => (
    <ExtensionSheet
      a={a}
      m={m}
      start={start}
      onDone={(r) => {
        close();
        if (r) done(r);
      }}
    />
  ));
}

type Phase = { kind: "form" } | { kind: "confirm"; confirm: ExtensionConfirm; round: number };

function initialValues(a: MessageAction, m: MessageView): Record<string, string> {
  return Object.fromEntries(
    a.action.form.map((f) => {
      const d = fillTemplate(f.default ?? "", m);
      if (f.type === "checkbox") return [f.id, d === "true" ? "true" : "false"];
      if (f.type === "select") return [f.id, f.options.some((o) => o.value === d) ? d : (f.options[0]?.value ?? "")];
      return [f.id, d];
    }),
  );
}

/** One choice needs no picking; any other number does. */
function soleChoice(c: ExtensionConfirm | null | undefined): string | null {
  return c?.choices.length === 1 ? c.choices[0].value : null;
}

function ExtensionSheet({ a, m, start, onDone }: { a: MessageAction; m: MessageView; start: SheetStart; onDone: (r: ExtensionResult | null) => void }) {
  const { ext, action } = a;
  const [values, setValues] = useState<Record<string, string>>(() => start?.form ?? initialValues(a, m));
  const [phase, setPhase] = useState<Phase>(start ? { kind: "confirm", confirm: start.confirm, round: 1 } : { kind: "form" });
  const [choice, setChoice] = useState<string | null>(() => soleChoice(start?.confirm));
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstRef = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  const confirm = phase.kind === "confirm" ? phase.confirm : null;
  const missing = action.form.filter((f) => f.required && f.type !== "checkbox" && !values[f.id]?.trim());
  const ready = !running && (confirm ? confirm.choices.length === 0 || choice !== null : missing.length === 0);
  const put = (id: string, v: string) => {
    setValues((s) => ({ ...s, [id]: v }));
    setError(null);
  };

  const run = async () => {
    if (!ready) return;
    setRunning(true);
    setError(null);
    try {
      const r = await api.runExtensionAction({
        extensionId: ext.id,
        actionId: action.id,
        accountId: m.accountId,
        messageId: m.id,
        form: values,
        continuation: phase.kind === "confirm" ? { state: phase.confirm.state, choice, round: phase.round } : undefined,
      });
      if (r.confirm) {
        // Penguin stops a fourth confirm (the backend errors), so this ends.
        setPhase({ kind: "confirm", confirm: r.confirm, round: phase.kind === "confirm" ? phase.round + 1 : 1 });
        setChoice(soleChoice(r.confirm));
        setRunning(false);
        bodyRef.current?.focus({ preventScroll: true });
        return;
      }
      onDone(r);
    } catch (e) {
      setError(asCommandError(e).message);
      setRunning(false);
    }
  };

  const onKey = (e: KeyboardEvent): boolean => {
    const t = e.target as HTMLElement | null;
    if (e.key === "Enter" && !e.isComposing) {
      // A text area takes Return as a new line and a focused button its own
      // press; ⌘Return runs from anywhere.
      if (!(e.metaKey || e.ctrlKey) && t?.closest("textarea, button, a")) return false;
      void run();
      return true;
    }
    if (confirm && confirm.choices.length > 0 && !running && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      const n = confirm.choices.length;
      const i = confirm.choices.findIndex((c) => c.value === choice);
      const next = i < 0 ? (e.key === "ArrowDown" ? 0 : n - 1) : (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
      setChoice(confirm.choices[next].value);
      return true;
    }
    return false;
  };

  const hasText = action.form.some((f) => f.type === "text" || f.type === "textarea");
  const hasTextarea = action.form.some((f) => f.type === "textarea");
  // A form that is one text area is a composer: give it room.
  const solo = action.form.length === 1 && hasTextarea;
  let focusedFirst = false;
  const title = confirm ? confirm.title : action.title;

  return (
    <Modal
      label={title}
      onClose={() => {
        if (!running) onDone(null);
      }}
      onKey={onKey}
      initialFocus={!confirm && hasText ? firstRef : bodyRef}
      className="ext-sheet"
    >
      <div ref={bodyRef} tabIndex={-1} className="ext-sheet-body">
        <header className="ext-head">
          <span className="ext-head-icon">
            <ExtensionIcon icon={action.icon} svg={action.iconSvg} size="sm" />
          </span>
          <div className="ext-head-text">
            <h3 className="ext-title">{title}</h3>
            <p className="ext-sub">{confirm ? (confirm.detail ?? ext.name) : ext.name}</p>
          </div>
        </header>

        {!confirm && (
          <div className="ext-context" title={m.subject}>
            <Icon name="mail" size="xs" />
            <span className="ext-context-subject truncate">{m.subject || "(no subject)"}</span>
            <span className="ext-context-from truncate">{m.from.name || m.from.email}</span>
          </div>
        )}

        <div className="modal-scroll ext-scroll">
          {confirm ? (
            <ConfirmBody confirm={confirm} choice={choice} onChoice={setChoice} disabled={running} />
          ) : (
            <div className="ext-fields">
              {action.form.map((f) => {
                const first = !focusedFirst && (f.type === "text" || f.type === "textarea") && (focusedFirst = true);
                const name = (
                  <span className="ext-label">
                    {f.label}
                    {f.required && <span className="ext-req"> *</span>}
                  </span>
                );
                if (f.type === "checkbox") {
                  return (
                    <label key={f.id} className="ext-check">
                      <input type="checkbox" checked={values[f.id] === "true"} disabled={running} onChange={(e) => put(f.id, e.target.checked ? "true" : "false")} />
                      {f.label}
                    </label>
                  );
                }
                if (f.type === "select") {
                  // A div, not a <label>: a label forwards clicks on it (the
                  // "outside" a user clicks to dismiss the list) to the Select's
                  // button, which opens the list again. Select names itself.
                  return (
                    <div key={f.id} className="ext-field">
                      {name}
                      <Select label={f.label} value={values[f.id]} options={f.options} disabled={running} onChange={(v) => put(f.id, v)} />
                    </div>
                  );
                }
                return (
                  <label key={f.id} className="ext-field">
                    {name}
                    {f.type === "textarea" ? (
                      <textarea
                        ref={first ? firstRef : undefined}
                        className={"input ext-textarea" + (solo ? " is-solo" : "")}
                        rows={solo ? 8 : 4}
                        placeholder={f.placeholder ?? undefined}
                        value={values[f.id]}
                        readOnly={running}
                        onChange={(e) => put(f.id, e.target.value)}
                      />
                    ) : (
                      <input
                        ref={first ? firstRef : undefined}
                        className="input"
                        placeholder={f.placeholder ?? undefined}
                        value={values[f.id]}
                        readOnly={running}
                        onChange={(e) => put(f.id, e.target.value)}
                      />
                    )}
                  </label>
                );
              })}
            </div>
          )}
        </div>

        {error && (
          <p className="ext-error" role="alert">
            <Icon name="info" size="xs" />
            <span>{error}</span>
          </p>
        )}
      </div>

      <footer className="ext-foot">
        <span className="hint ext-hint" aria-hidden="true">
          <Keys keys={confirm || !hasTextarea ? "enter" : "mod+enter"} />
          {confirm ? "to confirm" : "to run"}
        </span>
        <button type="button" className="btn btn-ghost" disabled={running} onClick={() => onDone(null)}>
          Cancel
        </button>
        <button type="button" className="btn btn-primary ext-run" disabled={!ready} aria-busy={running || undefined} onClick={() => void run()}>
          {running && <span className="spinner ext-spinner" aria-hidden="true" />}
          {running ? "Running…" : confirm ? (confirm.confirmLabel ?? "Continue") : action.title}
        </button>
      </footer>
    </Modal>
  );
}

function ConfirmBody({
  confirm,
  choice,
  onChoice,
  disabled,
}: {
  confirm: ExtensionConfirm;
  choice: string | null;
  onChoice: (v: string) => void;
  disabled: boolean;
}) {
  const question = confirm.choiceLabel ?? "Choose one";
  return (
    <div className="ext-confirm">
      {confirm.steps.length > 0 && (
        <ol className="ext-steps">
          {confirm.steps.map((s, i) => (
            <li key={i} className="ext-step">
              <span className="ext-step-n">{i + 1}</span>
              <span className="ext-step-text">{s.text}</span>
              {s.url && (
                <button type="button" className="btn btn-ghost btn-sm btn-icon ext-step-link" aria-label="Open link" title={s.url} onClick={() => void api.openExternal(s.url!)}>
                  <Icon name="external" size="xs" />
                </button>
              )}
            </li>
          ))}
        </ol>
      )}
      {confirm.choices.length > 0 && (
        <div className="ext-field">
          <span className="ext-label">{question}</span>
          <div className="ext-choices" role="radiogroup" aria-label={question}>
            {confirm.choices.map((c) => {
              const on = c.value === choice;
              return (
                <div
                  key={c.value}
                  role="radio"
                  aria-checked={on}
                  aria-disabled={disabled || undefined}
                  className={"ext-choice" + (on ? " is-on" : "")}
                  // Focus stays on the sheet, so Return confirms after a click.
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => !disabled && onChoice(c.value)}
                >
                  <span className="ext-choice-mark">{on && <Icon name="check" size="xs" />}</span>
                  <span className="ext-choice-text">
                    <span className="ext-choice-label truncate">{c.label}</span>
                    {c.detail && <span className="ext-choice-detail truncate">{c.detail}</span>}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
