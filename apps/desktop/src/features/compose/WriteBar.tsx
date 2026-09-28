// Write with AI (⌘⇧J): the bar under the message. It works on the selection
// when there is one, else on everything written above the signature, or
// drafts the message from an instruction when nothing is written yet. What
// the model writes streams into the text as a suggestion (editor/aiSuggest.ts)
// until you accept it (↵), discard it (Esc) or try again. Apple's on-device
// model; nothing leaves the Mac.
import { useEffect, useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import type { AiTarget, BodyHandle } from "./editor/handle";
import { buildRequest, cancelWrite, newRunId, promptPlaceholder, runWrite, WRITE_PRESETS, writeErrorText, type WritePreset } from "./ai";
import { api } from "../../lib/api";

type Phase =
  | { kind: "prompt" }
  | { kind: "writing"; runId: string }
  | { kind: "done"; text: string }
  | { kind: "error"; message: string };

export interface WriteContext {
  accountId: string;
  threadId: string | null;
  reply: boolean;
  subject: string;
  recipients: string[];
  myName: string;
}

export function WriteBar({
  body,
  context,
  onClose,
  onBusy,
}: {
  body: BodyHandle;
  context: () => WriteContext;
  onClose: () => void;
  /** A suggestion is showing (or being written): sending waits for Accept or Discard. */
  onBusy: (busy: boolean) => void;
}) {
  const [target, setTarget] = useState<AiTarget | null>(() => body.aiTarget());
  const [instruction, setInstruction] = useState("");
  const [phase, setPhase] = useState<Phase>({ kind: "prompt" });
  const last = useRef<{ preset: WritePreset["action"] | null; instruction: string } | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const acceptRef = useRef<HTMLButtonElement>(null);
  const running = useRef<string | null>(null);

  // Load the model while the instruction is being typed (Apple: prewarm
  // when there's a second or more before the request).
  useEffect(() => {
    api.prewarmWriter().catch(() => {
      // Only a head start; the run itself reports problems.
    });
  }, []);

  // Leaving (Esc, close, send) stops the run and puts the text back.
  useEffect(
    () => () => {
      if (running.current) cancelWrite(running.current);
      body.aiClear();
      onBusy(false);
    },
    [],
  );

  useEffect(() => {
    onBusy(phase.kind === "writing" || phase.kind === "done");
    if (phase.kind === "done") acceptRef.current?.focus();
    if (phase.kind === "prompt" || phase.kind === "error") inputRef.current?.focus();
  }, [phase.kind]);

  const scope = target?.scope ?? "writing";
  const hasText = !!target?.text.trim();

  async function run(preset: WritePreset["action"] | null, typed: string) {
    // Keep the target from when the bar opened: focus moved to the bar, but
    // the editor's selection is still the one the user made.
    const tgt = target ?? body.aiTarget();
    if (!tgt) return;
    if (!target) setTarget(tgt);
    if (!preset && !typed.trim()) {
      inputRef.current?.focus();
      return;
    }
    if (preset && !tgt.text.trim()) return;
    last.current = { preset, instruction: typed };
    const runId = newRunId();
    running.current = runId;
    setPhase({ kind: "writing", runId });
    body.aiPreview(tgt, "", "writing");
    const ctx = context();
    try {
      const text = await runWrite(
        buildRequest({ runId, ...ctx, preset, instruction: typed, scope: tgt.scope, text: tgt.text }),
        (partial) => {
          if (running.current === runId) body.aiPreview(tgt, partial, "writing");
        },
      );
      if (running.current !== runId) return;
      running.current = null;
      if (!text.trim()) {
        body.aiClear();
        setPhase({ kind: "error", message: "The model didn't write anything. Try saying it differently." });
        return;
      }
      body.aiPreview(tgt, text, "done");
      setPhase({ kind: "done", text });
    } catch (e) {
      if (running.current !== runId) return;
      running.current = null;
      body.aiClear();
      const message = writeErrorText(e);
      setPhase(message ? { kind: "error", message } : { kind: "prompt" });
    }
  }

  function stop() {
    if (running.current) cancelWrite(running.current);
    running.current = null;
    body.aiClear();
    setPhase({ kind: "prompt" });
  }

  function accept() {
    if (phase.kind !== "done" || !target) return;
    body.aiApply(target, phase.text);
    onClose();
  }

  function discard() {
    body.aiClear();
    setTarget(body.aiTarget());
    setPhase({ kind: "prompt" });
  }

  function retry() {
    const l = last.current;
    if (!l) return;
    body.aiClear();
    void run(l.preset, l.instruction);
  }

  return (
    <div
      className={`cmp-write${phase.kind === "writing" ? " is-writing" : ""}`}
      role="group"
      aria-label="Write with AI"
      onKeyDown={(e) => {
        const mod = e.metaKey || e.ctrlKey;
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          if (phase.kind === "writing") stop();
          else if (phase.kind === "done") discard();
          else onClose();
          return;
        }
        if (phase.kind === "done" && e.key === "Enter" && !mod) {
          e.preventDefault();
          e.stopPropagation();
          accept();
          return;
        }
        // 1–4: a preset, while the field is empty.
        if ((phase.kind === "prompt" || phase.kind === "error") && !instruction && !mod && !e.altKey && hasText) {
          const p = WRITE_PRESETS.find((x) => x.key === e.key);
          if (p) {
            e.preventDefault();
            void run(p.action, "");
          }
        }
      }}
    >
      <div className="cmp-write-row">
        <Icon name="sparkles" size="xs" className="cmp-write-ico" />
        {phase.kind === "writing" ? (
          <span className="cmp-write-status" role="status">
            <span className="spinner" aria-hidden="true" />
            Writing{scope === "selection" ? " the selection" : ""}…
          </span>
        ) : phase.kind === "done" ? (
          <span className="cmp-write-status">Suggestion ready</span>
        ) : (
          <input
            ref={inputRef}
            className="cmp-input cmp-write-input"
            value={instruction}
            onChange={(e) => setInstruction(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                e.stopPropagation();
                void run(null, instruction);
              }
            }}
            placeholder={promptPlaceholder(scope, hasText, context().reply)}
            aria-label="What to write"
            maxLength={500}
            autoFocus
          />
        )}
        <span className="grow" />
        {phase.kind === "writing" ? (
          <button className="btn btn-ghost btn-sm" onClick={stop}>
            Stop <span className="kbd">Esc</span>
          </button>
        ) : phase.kind === "done" ? (
          <>
            <button className="btn btn-ghost btn-sm" onClick={retry}>
              <Icon name="refresh" size="xs" />
              Try again
            </button>
            <button className="btn btn-ghost btn-sm" onClick={discard}>
              Discard <span className="kbd">Esc</span>
            </button>
            <button ref={acceptRef} className="btn btn-primary btn-sm" onClick={accept}>
              Accept <span className="kbd">↵</span>
            </button>
          </>
        ) : (
          <>
            <button className="btn btn-primary btn-sm" onClick={() => void run(null, instruction)} disabled={!instruction.trim()}>
              Write <span className="kbd">↵</span>
            </button>
            <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title="Close (Esc)" aria-label="Close Write with AI">
              <Icon name="x" size="xs" />
            </button>
          </>
        )}
      </div>
      {(phase.kind === "prompt" || phase.kind === "error") && hasText && (
        <div className="cmp-write-presets" role="toolbar" aria-label={scope === "selection" ? "Change the selection" : "Change the message"}>
          <span className="faint cmp-write-scope">{scope === "selection" ? "Selection:" : "Whole message:"}</span>
          {WRITE_PRESETS.map((p) => (
            <button key={p.action} className="btn btn-ghost btn-sm cmp-write-preset" onClick={() => void run(p.action, "")}>
              {p.label}
              {!instruction && <span className="kbd">{p.key}</span>}
            </button>
          ))}
        </div>
      )}
      {phase.kind === "error" && (
        <div className="cmp-write-error" role="alert">
          <Icon name="info" size="xs" />
          {phase.message}
          {last.current && (
            <button className="btn btn-ghost btn-sm" onClick={retry}>
              Try again
            </button>
          )}
        </div>
      )}
      <div className="cmp-write-foot faint">Apple Intelligence, on this Mac. Check what it wrote before sending.</div>
    </div>
  );
}
