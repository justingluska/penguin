// Small building blocks for the setup guide: numbered instructions, Copy and
// Open buttons, callouts and the per-step troubleshooting drawer.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api } from "../../lib/api";
import { Icon, type IconName } from "../../components/Icon";

/** Copies `text`; shows "Copied" for a moment. */
export function CopyButton({ text, label }: { text: string; label?: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setState("copied");
    } catch {
      setState("failed");
    }
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState("idle"), 1600);
  };
  return (
    <button
      type="button"
      className={`onb-copy${state === "copied" ? " is-copied" : ""}`}
      onClick={() => void copy()}
      aria-label={`Copy ${label ?? text}`}
      title={state === "failed" ? "Couldn't copy. Select the text instead." : `Copy ${label ?? text}`}
    >
      <span className="onb-copy-text mono">{text}</span>
      <span className="onb-copy-state" aria-live="polite">
        {state === "copied" ? (
          <>
            <Icon name="check" size="2xs" /> Copied
          </>
        ) : state === "failed" ? (
          "Select it"
        ) : (
          <>
            <CopyGlyph /> Copy
          </>
        )}
      </span>
    </button>
  );
}

// Not in the shared icon set (Lucide "copy", same 24px grid and stroke).
function CopyGlyph() {
  return (
    <svg className="i i-2xs" viewBox="0 0 24 24" aria-hidden="true">
      <rect width="14" height="14" x="8" y="8" rx="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </svg>
  );
}

/** Opens a Google page in the default browser. */
export function OpenButton({ url, children, primary }: { url: string; children: ReactNode; primary?: boolean }) {
  return (
    <a
      className={`btn btn-sm ${primary ? "btn-secondary onb-open-primary" : "btn-ghost"} onb-open`}
      href={url}
      title={url}
      onClick={(e) => {
        e.preventDefault();
        void api.openExternal(url);
      }}
    >
      {children}
      <Icon name="external" size="xs" />
    </a>
  );
}

/** An ordered list of instructions with big step numbers. */
export function Steps({ children }: { children: ReactNode }) {
  return <ol className="onb-steps">{children}</ol>;
}

export function Step({ title, children, action }: { title: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <li className="onb-step">
      <div className="onb-step-body">
        <div className="onb-step-title">
          <span className="grow">{title}</span>
          {action}
        </div>
        {children && <div className="onb-step-detail">{children}</div>}
      </div>
    </li>
  );
}

/** What a field should contain, e.g. "App name → Penguin [Copy]". */
export function Field({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="onb-field">
      <span className="onb-field-name">{name}</span>
      <span className="onb-field-value">{children}</span>
    </div>
  );
}

/** A console label the user looks for, styled like a UI control. */
export function Ui({ children }: { children: ReactNode }) {
  return <span className="onb-ui">{children}</span>;
}

export function Callout({ tone = "info", icon, title, children }: { tone?: "info" | "warn" | "ok"; icon?: IconName; title?: ReactNode; children: ReactNode }) {
  const ic: IconName = icon ?? (tone === "warn" ? "info" : tone === "ok" ? "check" : "info");
  return (
    <div className={`onb-callout is-${tone}`}>
      <Icon name={ic} size="sm" />
      <div>
        {title && <div className="onb-callout-title">{title}</div>}
        <div className="onb-callout-body">{children}</div>
      </div>
    </div>
  );
}

export interface Problem {
  /** What the user sees, e.g. an error code. */
  symptom: ReactNode;
  fix: ReactNode;
}

/** Collapsible "Troubleshooting" list for a step. */
export function Troubleshooting({ problems, open, onToggle }: { problems: Problem[]; open?: boolean; onToggle?: (open: boolean) => void }) {
  if (!problems.length) return null;
  return (
    <details className="onb-trouble" open={open} onToggle={(e) => onToggle?.((e.currentTarget as HTMLDetailsElement).open)}>
      <summary>
        <Icon name="right" size="xs" className="onb-trouble-chev" />
        Troubleshooting
        <span className="onb-trouble-count">{problems.length}</span>
      </summary>
      <dl>
        {problems.map((p, i) => (
          <div className="onb-problem" key={i}>
            <dt>{p.symptom}</dt>
            <dd>{p.fix}</dd>
          </div>
        ))}
      </dl>
    </details>
  );
}

export function Code({ children }: { children: ReactNode }) {
  return <code className="onb-code">{children}</code>;
}
