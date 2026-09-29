// Autosave to real Gmail drafts. One saver per open draft, kept in a module map
// so it outlives the composer (a close flushes in the background, and reopening
// the same draft picks the saver back up).
//
// Rules from the backend: the first save creates the draft and must finish
// before any other save (or a second draft is made), so every save for a draft
// runs through one promise chain. The draftId never changes; if the draft was
// deleted elsewhere the backend recreates it and returns a new id, so the
// latest returned id always wins.
import { useSyncExternalStore } from "react";
import { api, asCommandError } from "../../lib/api";
import { attachmentKey, toDraft, type EditorState } from "./draft";
import type { OutgoingAttachment } from "../../lib/types";

export type SaveStatus =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved"; at: number }
  | { kind: "error"; message: string; offline: boolean };

const DEBOUNCE_MS = 1800;
const RETRY_MS = 10_000;

function hasContent(s: EditorState): boolean {
  return s.to.length + s.cc.length + s.bcc.length > 0 || s.subject.trim() !== "" || s.body.trim() !== "" || s.attachments.length > 0;
}

export class DraftSaver {
  state: EditorState;
  status: SaveStatus = { kind: "idle" };
  /** Everything for this draft runs in order through this chain. */
  private chain: Promise<void> = Promise.resolve();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastSaved: string | null = null;
  private dead = false;
  private subs = new Set<() => void>();
  /**
   * After a save Gmail re-ids the draft's attachments; the backend returns
   * refs to the new message. Swapping them in (by file identity) means later
   * saves don't ship the bytes over IPC again.
   */
  private repointed = new Map<string, OutgoingAttachment>();

  constructor(state: EditorState) {
    this.state = state;
    // A reopened draft starts out identical to what the server has.
    if (state.draftId) this.lastSaved = fingerprint(state);
    if (state.draftId) this.status = { kind: "saved", at: Date.now() };
  }

  get draftId() {
    return this.state.draftId;
  }

  subscribe = (cb: () => void): (() => void) => {
    this.subs.add(cb);
    return () => {
      this.subs.delete(cb);
    };
  };

  private set(status: SaveStatus) {
    this.status = status;
    this.subs.forEach((f) => f());
  }

  /** Record an edit; saves ~2 s after typing stops. */
  update(state: EditorState) {
    // draftId/draftAccountId are owned by the saver, never by the editor.
    this.state = {
      ...state,
      draftId: this.state.draftId,
      draftAccountId: this.state.draftAccountId,
      attachments: this.withRepointed(state.attachments),
    };
    this.schedule(DEBOUNCE_MS);
  }

  private withRepointed(list: OutgoingAttachment[]): OutgoingAttachment[] {
    return list.map((a) => this.repointed.get(attachmentKey(a)) ?? a);
  }

  private schedule(ms: number) {
    if (this.dead) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, ms);
  }

  /** The latest editor state with attachments swapped for saved refs (for sending). */
  current(): EditorState {
    return { ...this.state, attachments: this.withRepointed(this.state.attachments) };
  }

  /** Save now (on close, before send). Resolves once every queued save is done. */
  flush(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.chain = this.chain.then(() => this.saveOnce());
    return this.chain;
  }

  private async saveOnce() {
    if (this.dead) return;
    const s = this.state;
    if (!hasContent(s) && !s.draftId) return;
    const fp = fingerprint(s);
    if (fp === this.lastSaved && s.draftAccountId === s.accountId) return;
    this.set({ kind: "saving" });
    try {
      // Gmail drafts belong to one account: after a From switch, the old draft
      // is removed and the next save creates it on the new account.
      if (s.draftId && s.draftAccountId && s.draftAccountId !== s.accountId) {
        await api.deleteDraft(s.draftAccountId, s.draftId);
        this.state = { ...this.state, draftId: null, draftAccountId: null };
      }
      const draft = toDraft(this.state);
      const sent = draft.attachments ?? [];
      const ref = await api.saveDraft(draft, this.state.draftId);
      // The refs come back in the order the attachments were sent.
      const refs = ref.attachments;
      if (refs && refs.length === sent.length) {
        this.repointed = new Map(sent.map((a, i) => [attachmentKey(a), refs[i]]));
      }
      this.state = {
        ...this.state,
        draftId: ref.draftId,
        draftAccountId: s.accountId,
        attachments: this.withRepointed(this.state.attachments),
      };
      this.lastSaved = fp;
      this.set({ kind: "saved", at: Date.now() });
    } catch (e) {
      const err = asCommandError(e);
      // The text stays in memory; try again shortly.
      this.set({ kind: "error", message: err.message, offline: err.code === "network" });
      this.schedule(RETRY_MS);
    }
  }

  /** What the server has is this draft as it is now. */
  isSaved(): boolean {
    return !!this.state.draftId && fingerprint(this.state) === this.lastSaved && this.state.draftAccountId === this.state.accountId;
  }

  /**
   * The server copy may be behind (its last save failed in another window):
   * the next flush saves even without an edit.
   */
  markUnsaved() {
    this.lastSaved = null;
    if (this.status.kind === "saved") this.set({ kind: "idle" });
  }

  /** Stopped for good (sent, discarded, or handed to another window). */
  get stopped(): boolean {
    return this.dead;
  }

  /** Stop saving and delete the server copy (Discard). */
  async discard(): Promise<void> {
    this.dead = true;
    if (this.timer) clearTimeout(this.timer);
    await this.chain;
    const { draftId, draftAccountId, accountId } = this.state;
    if (draftId) await api.deleteDraft(draftAccountId ?? accountId, draftId);
  }

  /** Stop saving without touching the server copy (after a send). */
  stop() {
    this.dead = true;
    if (this.timer) clearTimeout(this.timer);
  }
}

function fingerprint(s: EditorState): string {
  const d = toDraft(s);
  // Attachments by identity, not form: a file swapped for its ref is unchanged.
  // bodyHtml too: making a word bold changes no text but must still save.
  return JSON.stringify([d.accountId, d.to, d.cc, d.bcc, d.subject, d.bodyText, d.bodyHtml, (d.attachments ?? []).map(attachmentKey)]);
}

/** Savers by editor key ("new", "reply:<acc>/<thread>", "draft:<id>"). */
const savers = new Map<string, DraftSaver>();

export function saverFor(state: EditorState): DraftSaver {
  const existing = savers.get(state.key) ?? (state.draftId ? saverForDraftId(state.draftId) : undefined);
  if (existing) return existing;
  const s = new DraftSaver(state);
  savers.set(state.key, s);
  return s;
}

/** The saver for an editor key, if any (e.g. the "new" slot). */
export function saverForKey(key: string): DraftSaver | undefined {
  return savers.get(key);
}

/** The live saver for a Gmail draft, if one is open or still flushing. */
export function saverForDraftId(draftId: string): DraftSaver | undefined {
  for (const s of savers.values()) if (s.draftId === draftId) return s;
  return undefined;
}

/** Move a saver (and its editor state) to a new key, e.g. "new" → "draft:<id>". */
export function rekeySaver(s: DraftSaver, key: string) {
  forgetSaver(s);
  s.state = { ...s.state, key };
  savers.set(key, s);
}

/** Save every open draft now (before the page reloads, e.g. switching demo mode). */
export function flushAllSavers(): Promise<void> {
  return Promise.all([...new Set(savers.values())].map((s) => s.flush())).then(() => undefined);
}

/**
 * Drafts still open here whose latest text isn't on the server (saving
 * failed, e.g. offline), after flushAllSavers: a window about to close hands
 * these to the main window instead of losing them.
 */
export function unsavedDrafts(): EditorState[] {
  // Untouched composers (a reply opened and closed) were never meant to be drafts.
  return [...new Set(savers.values())].filter((s) => !s.stopped && s.state.touched && !s.isSaved() && hasContent(s.state)).map((s) => s.current());
}

export function forgetSaver(s: DraftSaver) {
  for (const [k, v] of savers) if (v === s) savers.delete(k);
}

export function useSaveStatus(s: DraftSaver | null): SaveStatus {
  return useSyncExternalStore(
    (cb) => (s ? s.subscribe(cb) : () => {}),
    () => (s ? s.status : IDLE),
  );
}
const IDLE: SaveStatus = { kind: "idle" };
