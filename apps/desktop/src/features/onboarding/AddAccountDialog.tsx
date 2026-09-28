// The Add account dialog (AddAccountModal.tsx opens it): its own chunk, with
// the setup flow it runs.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import type { Account } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { meta, setAccounts } from "../../app/store";
import { useAddFlow } from "./flow";
import { WhoChip } from "./ProviderSteps";
import { addAccountResume, notifyAccountAdded } from "./AddAccountModal";
import "./onboarding.css";

export function AddAccountDialog({ onClose }: { onClose: () => void }) {
  const all = meta.use((m) => m.accounts);
  const [added, setAdded] = useState<Account[]>([]);
  const onAdded = (a: Account) => {
    setAdded((list) => [...list.filter((x) => x.id !== a.id), a]);
    api.listAccounts().then(
      (list) => {
        setAccounts(list);
        notifyAccountAdded(a);
      },
      () => {},
    );
  };
  const [resume] = useState(addAccountResume);
  const flow = useAddFlow({ surface: "modal", accounts: added, allAccounts: all, onAdded, resume });
  const { step, steps, index } = flow;
  const headingRef = useRef<HTMLHeadingElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  const next = () => {
    if (flow.next()) onClose();
  };

  useLayoutEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
    if (step.id !== "email") headingRef.current?.focus({ preventScroll: true });
  }, [step.id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Own the keyboard while open: Esc closes, nothing reaches mail shortcuts.
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key === "Enter" && !e.defaultPrevented && !e.metaKey && !e.ctrlKey && !e.altKey && !e.isComposing) {
        const t = e.target as HTMLElement | null;
        if (!t?.closest("button, a, textarea, summary, select, form")) {
          e.preventDefault();
          e.stopPropagation();
          if (e.shiftKey) flow.back();
          else next();
          return;
        }
      }
      if (e.key !== "Tab" && e.key !== "Enter" && e.key !== " ") e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  return (
    <div className="onb-modal-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`onb-modal${step.wide ? " is-wide" : ""}`} role="dialog" aria-modal="true" aria-labelledby="onb-modal-title">
        <header className="onb-modal-head">
          <h2 id="onb-modal-title">Add account</h2>
          <span className="grow" />
          <button className="btn btn-ghost btn-sm btn-icon" aria-label="Close" onClick={onClose}>
            <Icon name="x" size="sm" />
          </button>
        </header>
        <div className="onb-modal-body" ref={bodyRef} key={step.id}>
          {flow.loadError ? (
            <div className="onb-error" role="alert">
              <Icon name="info" size="xs" />
              <span>Couldn't read Penguin's setup state: {flow.loadError}</span>
            </div>
          ) : flow.loaded ? (
            <>
              {step.id !== "email" && step.id !== "done" && (
                <div className="onb-eyebrow">
                  Step {index + 1} of {steps.length}
                </div>
              )}
              {step.heading && (
                <h3 className="onb-modal-title" ref={headingRef} tabIndex={-1}>
                  {step.heading}
                </h3>
              )}
              {step.lead && <p className="onb-modal-lead">{step.lead}</p>}
              {step.chip && flow.detected && <WhoChip detected={flow.detected} onChange={flow.restart} />}
              <div className="onb-content onb-modal-content">{step.content}</div>
            </>
          ) : null}
        </div>
        <footer className="onb-modal-foot">
          {index > 0 && step.id !== "done" && (
            <button className="btn btn-ghost" onClick={flow.back}>
              <Icon name="left" size="sm" />
              Back
            </button>
          )}
          <span className="grow" />
          {step.id !== "email" && !flow.isLast && (
            <button className="btn btn-ghost" onClick={onClose}>
              Cancel
            </button>
          )}
          {/* A connect step's own button (Sign in, Connect) is the way forward. */}
          {!(step.blocked && step.id !== "email") && (
            <button className="btn btn-primary" onClick={next} disabled={!flow.loaded || !!step.blocked}>
              {step.nextLabel ?? "Next"}
              <span className="kbd">↵</span>
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
