// Keyboard hints — the design system's most repeated element (.kbd).
import { Fragment, type ReactNode } from "react";
import { formatKeys } from "../lib/keyboard";

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={className ? "kbd " + className : "kbd"}>{children}</span>;
}

/** Render a registry key spec ("g i", "mod+k", "shift+u") as key caps. */
export function Keys({ keys, then = true }: { keys: string; then?: boolean }) {
  const chords = formatKeys(keys);
  if (chords.length === 1 && chords[0].length === 1) return <Kbd>{chords[0][0]}</Kbd>;
  return (
    <span className="kbd-group">
      {chords.map((caps, i) => (
        <Fragment key={i}>
          {i > 0 && then && <span className="kbd-then">then</span>}
          {caps.map((c, j) => (
            <Kbd key={j}>{c}</Kbd>
          ))}
        </Fragment>
      ))}
    </span>
  );
}
