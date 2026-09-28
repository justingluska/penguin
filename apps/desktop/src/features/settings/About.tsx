// Settings → About: who makes Penguin, where the code lives, and the version.
import { useEffect, useState } from "react";
import { Section } from "./parts";
import { api } from "../../lib/api";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { PenguinMark } from "../../components/PenguinMark";

const X_URL = "https://x.com/gluska";
const GITHUB_URL = "https://github.com/justingluska/penguin";

function open(url: string) {
  api.openExternal(url).catch((e) => toast({ tone: "error", message: "Couldn't open the link", detail: String(e?.message ?? e) }));
}

export function AboutSection() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    api.diagnostics().then(
      (d) => live && setVersion(d.appVersion),
      () => undefined, // the version line is optional; the rest of the page stands on its own
    );
    return () => {
      live = false;
    };
  }, []);

  return (
    <Section id="about" icon="info" title="About">
      <div className="about-hero">
        <PenguinMark className="logo about-logo" />
        <div>
          <div className="about-name">Penguin</div>
          <div className="st-muted tnum">{version ? `Version ${version}` : " "}</div>
        </div>
      </div>
      <p className="about-tagline">
        Fast, keyboard-first email that lives on your Mac. Your mail, search and AI stay on this computer: no Penguin servers, ever.
      </p>
      <div className="setting-row">
        <div>
          <span className="setting-label">Made by Justin Gluska</span>
          <p className="st-muted">Say hi, send ideas, or report a bug.</p>
        </div>
        <button className="btn btn-secondary btn-sm" onClick={() => open(X_URL)}>
          @gluska on X
          <Icon name="external" size="xs" />
        </button>
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Source available</span>
          <p className="st-muted">Read every line, file an issue, or build it yourself. Free to use under the PolyForm Shield license. Bring your own sign-in keys; nothing phones home.</p>
        </div>
        <button className="btn btn-secondary btn-sm" onClick={() => open(GITHUB_URL)}>
          GitHub
          <Icon name="external" size="xs" />
        </button>
      </div>
      <p className="about-footer st-muted">Built by one person and a flock of agents. Waddle on. 🐧</p>
    </Section>
  );
}
