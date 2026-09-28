import { useId } from "react";

// The in-app Penguin mark: the app icon, "Tuxedo split A"
// (design/icons/round3/tuxedo-split-a.svg), with the viewBox cropped to the
// tile so it fills its box. Sized by the .logo class (26px by default).
export function PenguinMark({ className = "logo" }: { className?: string }) {
  // Two marks can be on screen at once (onboarding), so the clip id is unique.
  const clip = useId();
  const tile =
    "M324 100H700C852 100 924 172 924 324V700C924 852 852 924 700 924H324C172 924 100 852 100 700V324C100 172 172 100 324 100Z";
  return (
    <svg className={className} viewBox="100 100 824 824" role="img" aria-label="Penguin">
      <defs>
        <clipPath id={clip}>
          <path d={tile} />
        </clipPath>
      </defs>
      <path d={tile} fill="#182B3A" />
      <path
        clipPath={`url(#${clip})`}
        fill="#FFFFFF"
        d="M340.499 381.957L587.721 485.499L1583.221 -466.597L1900 1900H1500L694 970C414.189 811.521 296.049 594.711 340.499 381.957Z"
      />
    </svg>
  );
}
