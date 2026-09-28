// penguin.log lines (tracing's default format) → entries for the log viewer.
//   2026-09-25T08:03:40.551290Z  WARN span{a=1}: penguin_ui: text key=value
// A line that doesn't start with a timestamp (panic output, a wrapped
// message) belongs to the entry above it.

export type LogLevel = "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE";

export interface LogEntry {
  /** Milliseconds since the epoch; null for lines before the first timestamp. */
  at: number | null;
  level: LogLevel | null;
  /** Module path ("penguin_gmail::sync"); "penguin_ui" is what the UI reported. */
  target: string;
  text: string;
}

const HEAD = /^(\d{4}-\d\d-\d\dT[\d:.]+Z)\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)$/;
const MODULE = /^[A-Za-z_]\w*(?:::\w+)*$/;

export function parseLog(lines: string[]): LogEntry[] {
  const out: LogEntry[] = [];
  for (const line of lines) {
    const m = HEAD.exec(line);
    if (!m) {
      const prev = out[out.length - 1];
      if (prev) prev.text += "\n" + line;
      else if (line.trim()) out.push({ at: null, level: null, target: "", text: line });
      continue;
    }
    const [, ts, level, rest] = m;
    // Spans come first ("name{fields}: "); the target is the first module path followed by ": ".
    const parts = rest.split(": ");
    let target = "";
    let text = rest;
    for (let i = 0; i < parts.length - 1; i++) {
      if (MODULE.test(parts[i])) {
        target = parts[i];
        text = parts.slice(i + 1).join(": ");
        break;
      }
    }
    const at = Date.parse(ts);
    out.push({ at: Number.isNaN(at) ? null : at, level: level as LogLevel, target, text });
  }
  return out;
}

/** Warnings and errors: the "Problems" filter. */
export const isProblem = (e: LogEntry) => e.level === "ERROR" || e.level === "WARN";
