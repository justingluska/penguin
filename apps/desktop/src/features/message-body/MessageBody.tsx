// OWNER: security-render agent. Renders one message's sanitized HTML inside a
// sandboxed iframe (srcdoc, no scripts), auto-sized to content, with a
// "Remote images blocked · Load images" bar. Used by the thread view.
//
// Isolation model (see docs/SECURITY.md):
// - sandbox has NO allow-scripts, so nothing in the message can run, and the
//   frame has no forms, no top navigation, and no plugins.
// - allow-same-origin is set WITHOUT allow-scripts. That combination is safe:
//   the "sandbox escape" warning only applies when both are present, because
//   frame script could then remove its own sandbox. Here the only script
//   touching the frame is ours, from the parent, and we need same-origin to
//   read the content height (a sandboxed opaque-origin frame is unreadable,
//   which would mean a fixed height with nested scrolling for every message).
// - Links: WebKit (macOS) never runs parent listeners on a no-script frame,
//   so the click handler below only helps on Chromium-based webviews. Every
//   link is target=_blank and the sandbox allows popups, so a click becomes a
//   new-window navigation that the Rust link guard (src-tauri) cancels and
//   hands to the system browser. Nothing ever navigates inside the app.
// - Pictures: once loaded, image-viewer/decorate.ts wraps each picture in our
//   own penguin-image:// anchor (or gives a linked one a corner button), with
//   a per-document nonce registered in image-viewer/bridge.ts. A click comes
//   back through the same link guard (or the in-frame listener on Chromium)
//   and opens the image viewer; see docs/SECURITY.md → "Image viewer".
// - The frame's CSP (meta tag in the document) plus the app CSP it inherits
//   block scripts, remote fetches (until images are allowed), forms and base.
//
// "Dark email bodies" (experimental setting): in the dark theme, darkBody.ts
// darkens the loaded HTML document from here; "Show original colors" in the
// privacy row undoes it for one message.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Icon } from "../../components/Icon";
import { api } from "../../lib/api";
import type { MessageView } from "../../lib/types";
import { setTrustedImageSender, useSetting } from "../../lib/settings";
import { toast } from "../../components/Toast";
import { darken, restore, type DarkMode } from "./darkBody";
import { openPrivacyDetails } from "./PrivacyDialog";
import { privacySummary } from "./privacy";
import { newNonce, registerFrame } from "../image-viewer/bridge";
import { decorate, viewerAnchor, type DecorateState } from "../image-viewer/decorate";
import { openFromRequest } from "../image-viewer/ImageViewer";
import { canSaveAll, frameBodyImages, messageItems, type BodyImage } from "../image-viewer/items";
import { saveAllImages } from "../image-viewer/actions";
import "./MessageBody.css";

// Must match HTML_DOC_PREFIX / TEXT_DOC_PREFIX in crates/penguin-render.
const HTML_PREFIX = '<!DOCTYPE html><html class="pg-html">';
const TEXT_PREFIX = '<!DOCTYPE html><html class="pg-text">';

/** Taller messages scroll inside the frame instead of growing it further. */
const MAX_HEIGHT = 40000;
const MIN_HEIGHT = 20;

type Kind = "html" | "text" | "other";
type Theme = "light" | "dark";

function kindOf(html: string): Kind {
  if (html.startsWith(HTML_PREFIX)) return "html";
  if (html.startsWith(TEXT_PREFIX)) return "text";
  return "other";
}

function appTheme(): Theme {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

/** Hides an HTML document that is about to be darkened, on the dark canvas,
 * so its light colors never paint; syncScheme() removes it. Visibility
 * doesn't change layout, so measuring works meanwhile. */
const DARK_PENDING = "visibility:hidden;background:#1c1c1e";

/** Plain-text documents follow the app theme; stamp it into the prefix so the
 * first paint is already right (no flash). `theme` is one of two literals.
 * An HTML document that will be darkened starts hidden (DARK_PENDING). */
function withTheme(html: string, kind: Kind, theme: Theme, darkPending: boolean): string {
  if (kind === "html" && darkPending) return HTML_PREFIX.replace("<html ", `<html style="${DARK_PENDING}" `) + html.slice(HTML_PREFIX.length);
  if (kind !== "text") return html;
  return TEXT_PREFIX.replace("<html ", `<html data-theme="${theme}" `) + html.slice(TEXT_PREFIX.length);
}

function isExternalUrl(href: string): boolean {
  return /^(https?:|mailto:)/i.test(href.trim());
}

/** Content height of the frame's document. Measures our root wrapper (or the
 * body for documents we didn't produce) rather than documentElement, whose
 * scrollHeight never drops below the frame's own height and so could only
 * ever grow. */
function contentHeight(doc: Document): number | null {
  const root = (doc.querySelector(".pg-root") as HTMLElement | null) ?? doc.body;
  if (!root) return null;
  const rect = root.getBoundingClientRect();
  const view = doc.defaultView;
  const marginBottom = view ? parseFloat(view.getComputedStyle(root).marginBottom) || 0 : 0;
  return Math.ceil(rect.top + Math.max(rect.height, root.scrollHeight) + marginBottom + (doc.scrollingElement?.scrollTop ?? 0));
}

/** `trailing`: shown at the right end of the privacy row (the Unsubscribe
 * button); the row then shows even with nothing blocked. */
export function MessageBody({ message, onLoadImages, trailing }: { message: MessageView; onLoadImages?: () => void; trailing?: ReactNode }) {
  const frameRef = useRef<HTMLIFrameElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(MIN_HEIGHT);
  const [theme, setTheme] = useState<Theme>(appTheme);
  const [loadingImages, setLoadingImages] = useState(false);
  const kind = kindOf(message.html);
  const darkBodies = useSetting("darkEmailBodies");
  // "Show original colors" for this message (not persisted).
  const [original, setOriginal] = useState(false);
  const [darkMode, setDarkMode] = useState<DarkMode | null>(null);
  const darkWanted = kind === "html" && darkBodies && theme === "dark";
  const wantDark = darkWanted && !original;
  const wantDarkRef = useRef(wantDark);
  wantDarkRef.current = wantDark;
  // The image viewer reads the latest render's message.
  const messageRef = useRef(message);
  messageRef.current = message;
  // The pictures decorated in the loaded body (for "Save all images").
  const [bodyImages, setBodyImages] = useState<BodyImage[]>([]);
  const syncBodyImages = (next: BodyImage[]) =>
    setBodyImages((prev) => (prev.length === next.length && prev.every((p, i) => p.index === next[i].index && p.src === next[i].src) ? prev : next));
  const imageItems = useMemo(() => messageItems(message, bodyImages), [message, bodyImages]);

  // Theme is read when the document changes, not on every theme change:
  // changing srcdoc reloads the frame. Live theme changes are applied to the
  // loaded document below instead.
  const srcDoc = useMemo(() => withTheme(message.html, kind, appTheme(), wantDarkRef.current), [message.html, kind]);

  useEffect(() => setLoadingImages(false), [message.html]);
  useEffect(() => setOriginal(false), [message.id]);

  // Darken or restore the loaded document to match wantDark, and reveal it
  // if it was stamped hidden. Called when a document is ready and whenever
  // wantDark changes.
  const syncScheme = useRef((_doc: Document) => {});
  syncScheme.current = (doc: Document) => {
    const root = doc.documentElement;
    const pending = root.style.visibility === "hidden";
    try {
      // The pending background would hide the real one from the pass.
      if (pending) root.style.removeProperty("background");
      if (wantDarkRef.current) setDarkMode(darken(doc));
      else {
        restore(doc);
        setDarkMode(null);
      }
    } finally {
      if (pending) root.style.removeProperty("visibility");
    }
  };

  useEffect(() => {
    const mo = new MutationObserver(() => setTheme(appTheme()));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, []);

  // Size the frame to its content. The parent can read the same-origin
  // document (no script runs inside it); a ResizeObserver created here, in the
  // parent's realm, fires for layout changes inside the frame, including late
  // image loads and <details> folds opening. Verified in WKWebView.
  useEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    let ro: ResizeObserver | null = null;
    let observed: Document | null = null;
    let schemed: Document | null = null;
    let raf = 0;
    let tries = 0;
    // Image viewer: this document's nonce and decorated pictures.
    let pictures: DecorateState | null = null;
    let unregister: (() => void) | null = null;
    let decorateRaf = 0;
    const scheduleDecorate = () => {
      if (!pictures || decorateRaf) return;
      decorateRaf = requestAnimationFrame(() => {
        decorateRaf = 0;
        const doc = frame.contentDocument;
        if (pictures && doc && doc === observed) {
          decorate(doc, pictures);
          syncBodyImages(frameBodyImages({ images: pictures.images, doc }));
        }
      });
    };

    const measure = () => {
      const doc = frame.contentDocument;
      if (!doc) return;
      const h = contentHeight(doc);
      if (h == null) return;
      const clamped = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, h));
      setHeight((prev) => (Math.abs(prev - clamped) >= 1 ? clamped : prev));
      // Layout changed (late image, resize): new pictures, corner buttons moved.
      scheduleDecorate();
    };

    const onLinkClick = (e: MouseEvent) => {
      if (e.button > 1) return;
      // One of our picture anchors: open the viewer, never navigate. The
      // request must carry this document's own nonce (bridge.ts).
      const pic = viewerAnchor(e.target);
      if (pic) {
        e.preventDefault();
        if (e.type === "click" && e.button === 0) openFromRequest(pic.getAttribute("href"), { doc: pic.ownerDocument });
        return;
      }
      const target = e.target as Element | null;
      const a = target && typeof target.closest === "function" ? target.closest("a[href]") : null;
      if (!a) return;
      e.preventDefault();
      const href = a.getAttribute("href") ?? "";
      if (isExternalUrl(href)) {
        api.openExternal(href).catch((err) => console.warn("penguin: could not open link", err));
      }
    };

    const attach = () => {
      const doc = frame.contentDocument;
      // Before the srcdoc document commits, contentDocument is the initial
      // about:blank document; wait for ours.
      if (doc && doc.URL === "about:srcdoc" && doc.body) {
        if (doc !== observed) {
          observed = doc;
          ro?.disconnect();
          ro = new ResizeObserver(measure);
          ro.observe(doc.documentElement);
          const root = doc.querySelector(".pg-root");
          if (root) ro.observe(root);
          doc.addEventListener("click", onLinkClick, true);
          doc.addEventListener("auxclick", onLinkClick, true);
          if (doc.documentElement.classList.contains("pg-text")) doc.documentElement.dataset.theme = appTheme();
          unregister?.();
          syncBodyImages([]);
          pictures = { nonce: newNonce(), images: [] };
          unregister = registerFrame({ nonce: pictures.nonce, doc, frame, message: () => messageRef.current, images: pictures.images });
        }
        measure();
        // The dark pass needs the whole document parsed. The frame's load
        // event (which calls attach again) can wait on remote images, so poll.
        if (doc.readyState !== "loading") {
          if (doc !== schemed) {
            schemed = doc;
            syncScheme.current(doc);
          }
        } else if (tries++ < 600) raf = requestAnimationFrame(attach);
        return;
      }
      if (tries++ < 120) raf = requestAnimationFrame(attach);
    };

    const onLoad = () => attach();
    frame.addEventListener("load", onLoad);
    attach();
    return () => {
      cancelAnimationFrame(raf);
      cancelAnimationFrame(decorateRaf);
      unregister?.();
      frame.removeEventListener("load", onLoad);
      ro?.disconnect();
      observed?.removeEventListener("click", onLinkClick, true);
      observed?.removeEventListener("auxclick", onLinkClick, true);
    };
  }, [srcDoc]);

  // Live theme changes for plain-text messages (HTML mail: see below).
  useEffect(() => {
    const doc = frameRef.current?.contentDocument;
    if (doc?.documentElement?.classList.contains("pg-text")) doc.documentElement.dataset.theme = theme;
  }, [theme]);

  // Live changes of the theme, the setting or "Show original colors" for a
  // loaded HTML document (a document still loading is handled on attach).
  useEffect(() => {
    const doc = frameRef.current?.contentDocument;
    if (doc?.URL === "about:srcdoc" && doc.readyState !== "loading" && doc.documentElement?.classList.contains("pg-html")) {
      syncScheme.current(doc);
    }
  }, [wantDark]);

  // Keyboard-first: a click inside the message moves focus into the frame,
  // where the app's shortcuts can't hear keys (no script runs there). When
  // the pointer leaves the frame with no button held (so a drag-selection is
  // over), hand focus back to the app. Any selection inside the frame stays,
  // and the copy bridge below serves ⌘C from it.
  useEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const onLeave = (e: MouseEvent) => {
      if (e.buttons !== 0) return;
      if (document.activeElement === frame) wrapRef.current?.focus({ preventScroll: true });
    };
    frame.addEventListener("mouseleave", onLeave);
    return () => frame.removeEventListener("mouseleave", onLeave);
  }, []);

  useEffect(() => {
    const onCopy = (e: ClipboardEvent) => {
      const frameSel = frameRef.current?.contentWindow?.getSelection();
      if (!frameSel || frameSel.isCollapsed) return;
      const own = document.getSelection();
      if (own && !own.isCollapsed) return;
      // Plain text only: importing the frame's nodes into the app document
      // (for a text/html flavor) would let their images load outside the
      // frame's CSP.
      e.clipboardData?.setData("text/plain", frameSel.toString());
      e.preventDefault();
    };
    document.addEventListener("copy", onCopy);
    return () => document.removeEventListener("copy", onCopy);
  }, []);

  const blocked = message.blockedRemoteImages;
  // What was removed or cleaned here (trackers, link parameters); a tracker
  // that loaded because blocking is off shows as a warning.
  const summary = privacySummary(message);
  const summaryClass = "mb-privacy" + (summary?.tone === "warn" ? " is-warn" : "");
  // Settings → Privacy: "never" hides the button (the backend refuses too).
  const imagePolicy = useSetting("remoteImages");
  const canLoad = !!onLoadImages && imagePolicy !== "never";
  const trustSender = () => {
    // The settings change re-renders cached threads with this sender's images.
    setTrustedImageSender(message.from.email, true).then(
      () => toast({ message: `Images from ${message.from.email} will always load` }),
      (e) => toast({ tone: "error", message: `Couldn't save: ${e?.message ?? e}` }),
    );
  };
  const loadImages = () => {
    if (!onLoadImages || loadingImages) return;
    setLoadingImages(true);
    onLoadImages();
  };
  const showPrivacy = () => openPrivacyDetails(message, canLoad && !loadingImages ? loadImages : undefined);
  // Only offered when darkening changed something (not for a message that
  // was already dark or too large to adapt).
  const schemeToggle =
    darkWanted && (original || darkMode === "native" || darkMode === "adapted") ? (
      <button
        type="button"
        className="mb-load mb-scheme"
        onClick={() => setOriginal((v) => !v)}
        title={original ? "Show this message dark" : "Show this message with the colors it was sent with"}
      >
        {original ? "Show dark colors" : "Show original colors"}
      </button>
    ) : null;
  // Two or more pictures (body pictures that loaded, image attachments).
  const saveAll = canSaveAll(imageItems) ? (
    <button
      type="button"
      className="mb-load mb-saveall"
      onClick={() => void saveAllImages(message, imageItems)}
      title="Save every image in this message to a new folder in Downloads"
    >
      <Icon name="download" size="sm" />
      <span>Save all images ({imageItems.length})</span>
    </button>
  ) : null;

  return (
    <div className="mb" ref={wrapRef} tabIndex={-1}>
      {blocked > 0 ? (
        <div className="mb-bar" role="status">
          <Icon name="image" size="sm" />
          <span>
            Remote images blocked <span className="mb-count">({blocked})</span>
          </span>
          {canLoad ? (
            <>
              <span className="mb-sep" aria-hidden="true">·</span>
              <button type="button" className="mb-load" onClick={loadImages} disabled={loadingImages}>
                {loadingImages ? "Loading…" : "Load images"}
              </button>
              {message.trustedSenderUnverified ? (
                <span className="mb-unverified">Couldn't verify this sender, so images weren't loaded automatically</span>
              ) : message.from.email && !message.labelIds.includes("SPAM") ? (
                // Not for spam: trusting its sender would load a spammer's pictures on every message.
                <button type="button" className="mb-load" onClick={trustSender} title={`Always load images from ${message.from.email}`}>
                  Always from this sender
                </button>
              ) : null}
            </>
          ) : null}
          {summary ? (
            <button type="button" className={"mb-trackers " + summaryClass} onClick={showPrivacy} title="What was removed and why">
              {summary.label}
            </button>
          ) : null}
          {saveAll}
          {schemeToggle}
          {trailing ? <span className="mb-trail">{trailing}</span> : null}
        </div>
      ) : summary || trailing || schemeToggle || saveAll ? (
        <div className="mb-bar is-quiet" role="status">
          {summary ? (
            <button type="button" className={summaryClass} onClick={showPrivacy} title="What was removed and why">
              <Icon name={summary.tone === "warn" ? "eye" : "shield"} size="sm" />
              <span>{summary.label}</span>
            </button>
          ) : null}
          {saveAll}
          {schemeToggle}
          {trailing ? <span className="mb-trail">{trailing}</span> : null}
        </div>
      ) : null}
      <iframe
        ref={frameRef}
        className={"mb-frame is-" + kind + (wantDark ? " is-dark" : "")}
        title={message.subject ? `Message: ${message.subject}` : "Message body"}
        // No allow-scripts. allow-same-origin only lets the parent measure
        // the document; allow-popups lets a link click reach the native
        // link guard, which opens it in the system browser.
        sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"
        referrerPolicy="no-referrer"
        // Out of the Tab order: keyboard focus belongs to the app's triage keys.
        tabIndex={-1}
        allow=""
        srcDoc={srcDoc}
        style={{ height }}
      />
    </div>
  );
}
