// Shared building blocks for the Settings screen. OWNER: settings agent.
import { useEffect, useRef, type ReactNode } from "react";
import type { SyncStatus } from "../../lib/types";
import { Icon, type IconName } from "../../components/Icon";
import type { SettingsSection } from "./state";

const PHASE_LABEL: Record<SyncStatus["phase"], string> = {
  idle: "Up to date",
  backfilling: "Downloading",
  incremental: "Syncing",
  error: "Error",
  needsReauth: "Sign in again",
};

export function phaseLabel(s: SyncStatus["phase"]): string {
  return PHASE_LABEL[s];
}

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------
export function Section({
  id,
  icon,
  title,
  badge,
  className,
  children,
}: {
  id: SettingsSection;
  icon: IconName;
  title: string;
  badge?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section id={`settings-${id}`} className={"settings-section" + (className ? " " + className : "")} aria-label={title}>
      <div className="st-heading">
        <Icon name={icon} size="sm" />
        <h2>{title}</h2>
        {badge}
      </div>
      {children}
    </section>
  );
}

export function Choice<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string; icon?: IconName }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="setting-choice" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={value === o.value}
          className={value === o.value ? "selected" : ""}
          onClick={() => onChange(o.value)}
        >
          {o.icon ? <Icon name={o.icon} size="xs" /> : null}
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Switch({ on, onChange, label, disabled }: { on: boolean; onChange?: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      className="st-switch"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange?.(!on)}
    >
      {on ? "On" : "Off"}
      <span className={"toggle" + (on ? " on" : "")}>
        <span />
      </span>
    </button>
  );
}

/** A small confirm dialog above Settings (its .st-confirm class makes Settings' keys back off). */
export function ConfirmDialog({
  title,
  body,
  confirm,
  onConfirm,
  onCancel,
}: {
  title: string;
  body: string;
  confirm: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    cancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onCancel]);
  return (
    <div className="st-scrim" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="st-confirm" role="alertdialog" aria-modal="true" aria-label={title}>
        <h3>{title}</h3>
        <p className="st-muted">{body}</p>
        <div className="st-confirm-actions">
          <button ref={cancelRef} className="btn btn-ghost" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-danger" onClick={onConfirm}>
            {confirm}
          </button>
        </div>
      </div>
    </div>
  );
}
