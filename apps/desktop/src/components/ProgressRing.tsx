// A small circular progress indicator: a ring that fills clockwise when the
// fraction is known, a spinning arc when it isn't.
export function ProgressRing({ value, size = 16, label }: { value: number | null; size?: number; label?: string }) {
  const stroke = size <= 16 ? 2 : 2.5;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const known = value != null;
  const filled = known ? Math.max(0, Math.min(1, value)) : 0.28;
  return (
    <svg
      className={"progress-ring" + (known ? "" : " is-indeterminate")}
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={known ? Math.round(filled * 100) : undefined}
    >
      <circle className="pr-track" cx={size / 2} cy={size / 2} r={r} strokeWidth={stroke} fill="none" />
      <circle
        className="pr-fill"
        cx={size / 2}
        cy={size / 2}
        r={r}
        strokeWidth={stroke}
        fill="none"
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={c * (1 - filled)}
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
      />
    </svg>
  );
}
