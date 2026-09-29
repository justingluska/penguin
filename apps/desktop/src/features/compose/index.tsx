// OWNER: ui-search agent. Mounted once by App; renders when ui.overlay === "compose".
//
// The composer (design/04-compose.html): From account picker with identity
// dots, recipient chips with autocomplete, Cc/Bcc, subject, a rich-text body
// (editor/RichBody.tsx, lazy-loaded) with ";" snippets and signatures, the
// quoted original for replies, ⌘↵ to send with a 10-second undo window. Esc
// closes and keeps the draft in memory.
import { lazy, Suspense, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import { getUi, setUi, useUi } from "../../lib/ui";
import type { Account, Address, ScheduledSend, ThreadRef } from "../../lib/types";
import { displayName } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { AccountAvatar, Avatar, accountName } from "../../components/Identity";
import { matchPeople, seedPeople, usePeople } from "../search/people";
import { applyDevParams } from "../search/dev";
import { preferredFromAccount, profileFirst } from "../../app/profiles";
import { listOrderedAccounts } from "../../app/store";
import {
  blankState,
  contextKey,
  drafts,
  parseAddress,
  pickAccount,
  prefill,
  loadOriginal,
  senderFromQuoteHeader,
  splitQuote,
  toDraft,
  attachmentSize,
  MAX_ATTACHMENT_BYTES,
  type ComposeContext,
  type EditorState,
} from "./draft";
import { queueSend } from "./send";
import { currentSettings, useSetting } from "../../lib/settings";
import { forgetSaver, rekeySaver, saverFor, saverForDraftId, saverForKey, useSaveStatus, type DraftSaver, type SaveStatus } from "./autosave";
import { AttachmentList, filesToAttachments, OriginalFiles, type OriginalStatus } from "./attachments";
import { hasAllFiles, withFiles, withoutFiles } from "./quote";
import { asFile, referencedCids, splitDropped } from "./inline";
import { loadInlineSrc, setInlineSrc } from "./inlineSrc";
import { docCids } from "./editor/serialize";
import { quoteFromHtml } from "./quoteDom";
import { clashingNames, RecipientChip } from "./RecipientChip";
import { fmtWhen, RemindMenu, remindLabel, RUNNING_NOTE, scheduleErrorText, SendLaterMenu, useOutboxToasts } from "./outbox";
import { recordSnippetUse, useSnippets } from "./snippetStore";
import { expandSnippet, snippetExtras, unfilledPlaceholders, type Snippet, type SnippetContext } from "./snippets";
import { SnippetPicker } from "./SnippetPicker";
import { InstantRow } from "./InstantReplies";
import { instantChoices, takeComposeIntent, wantsSuggestions, type InstantChoice } from "./instant";
import { WriteBar } from "./WriteBar";
import { checkWriter, useAiAvailable, useWriterReady } from "./ai";
import { registerComposeShortcuts, setComposerBus } from "./commands";
import "./compose.css";
import { useKeyTip } from "../../lib/shortcutHints";
import { useBodyHandle } from "./editor/handle";
import { autoInserts, defaultSignature, lockedAccount } from "./editor/pick";
import { useDismiss } from "../../lib/dismiss";
import { isMainWindow, windowRoute } from "../../lib/windowBus";
import { setThisWindowTitle } from "../../lib/windowChrome";
import { composeTitle, type ComposeSeed } from "./seed";

// The editor (TipTap) is its own chunk: App fetches it in the background once
// the first mail rows have painted (lib/lazy.ts prefetchScreens), so the first
// composer opens without waiting and neither launch nor the first rows pay for it.
const loadRichBody = () => import("./editor/RichBody");
const RichBody = lazy(loadRichBody);
export const preloadComposeEditor = loadRichBody;

export function Compose() {
  const overlay = useUi((s) => s.overlay);
  const ctx = useUi((s) => s.composeContext);
  useEffect(applyDevParams, []);
  useEffect(registerComposeShortcuts, []);
  // Scheduled-send and reminder toasts: once, in the main window.
  useOutboxToasts({ draft: (accountId, draftId) => void openDraftById(accountId, draftId) }, isMainWindow);
  // With native drag-drop off (so the composer gets File objects), a file
  // dropped anywhere else would make WebKit try to open it as the page. Swallow
  // those drops; the composer's own handlers take drops over it.
  useEffect(() => {
    const outside = (e: DragEvent) => {
      if (!e.dataTransfer?.types.includes("Files")) return;
      if ((e.target as Element | null)?.closest?.(".composer")) return;
      e.preventDefault();
      if (e.type === "dragover") e.dataTransfer.dropEffect = "none";
    };
    window.addEventListener("dragover", outside);
    window.addEventListener("drop", outside);
    return () => {
      window.removeEventListener("dragover", outside);
      window.removeEventListener("drop", outside);
    };
  }, []);
  if (overlay !== "compose") return null;
  const c: ComposeContext = ctx ?? { mode: "new" };
  return <Composer key={contextKey(c)} ctx={c} />;
}

/**
 * Reopen a saved Gmail draft (Drafts view, "Edit draft", the row chip).
 * `messageId` is the DRAFT message; without it the thread's latest one is used.
 */
export async function openDraftForThread(ref: ThreadRef, messageId?: string): Promise<void> {
  try {
    let msgId = messageId ?? null;
    if (!msgId) {
      const thread = await api.getThread(ref.accountId, ref.threadId);
      msgId = [...(thread?.messages ?? [])].reverse().find((m) => m.labelIds.includes("DRAFT"))?.id ?? null;
    }
    await openDraftVia(ref.accountId, msgId ? { messageId: msgId } : null);
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open the draft: ${asCommandError(e).message}` });
  }
}

/**
 * New message to `address`, from `accountId` (e.g. the account that usually
 * writes to them) or the usual default. Used by the person card.
 */
export async function openComposeTo(address: Address, accountId?: string): Promise<void> {
  try {
    const accs = profileFirst(await listOrderedAccounts());
    const ctx: ComposeContext = { mode: "new" };
    const key = contextKey(ctx);
    const existing = drafts.get(key);
    const slot = saverForKey(key);
    if (existing && existing.touched && !(slot?.draftId ?? existing.draftId)) {
      // An unsaved new message is still open in memory: add them to it rather
      // than lose it.
      if (!existing.to.some((a) => a.email.toLowerCase() === address.email.toLowerCase())) {
        drafts.set(key, { ...existing, to: [...existing.to, address] });
      }
      toast({ message: `Added ${displayName(address)} to the message you were writing` });
    } else {
      // Anything in the slot already lives in Gmail Drafts; start fresh.
      if (slot?.draftId) rekeySaver(slot, `draft:${slot.draftId}`);
      else if (slot) forgetSaver(slot);
      drafts.set(key, { ...blankState(ctx, pickAccount(accs, accountId, preferredFromAccount())), to: [address] });
    }
    // A composer already showing a new message must remount to pick this up.
    if (getUi().overlay === "compose" && getUi().composeContext?.mode === "new") {
      setUi({ overlay: null });
      requestAnimationFrame(() => setUi({ overlay: "compose", composeContext: ctx }));
    } else setUi({ overlay: "compose", composeContext: ctx });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't start a message: ${asCommandError(e).message}` });
  }
}

/**
 * A new message with recipient, subject and body filled in, from a given
 * account (the Unsubscribe button's "Edit first" for a mailto). An unsent new
 * message already in memory is kept and shown instead.
 */
export async function openComposeFilled(fill: { accountId: string; to: Address; subject: string; body: string }): Promise<void> {
  try {
    const accs = profileFirst(await listOrderedAccounts());
    const ctx: ComposeContext = { mode: "new" };
    const key = contextKey(ctx);
    const existing = drafts.get(key);
    const slot = saverForKey(key);
    if (existing && existing.touched && !(slot?.draftId ?? existing.draftId)) {
      toast({ tone: "error", message: "Finish or close the message you're writing first" });
      setUi({ overlay: "compose", composeContext: ctx });
      return;
    }
    if (slot?.draftId) rekeySaver(slot, `draft:${slot.draftId}`);
    else if (slot) forgetSaver(slot);
    drafts.set(key, { ...blankState(ctx, pickAccount(accs, fill.accountId, preferredFromAccount())), to: [fill.to], subject: fill.subject, body: fill.body });
    if (getUi().overlay === "compose" && getUi().composeContext?.mode === "new") {
      setUi({ overlay: null });
      requestAnimationFrame(() => setUi({ overlay: "compose", composeContext: ctx }));
    } else setUi({ overlay: "compose", composeContext: ctx });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't start a message: ${asCommandError(e).message}` });
  }
}

/** Reopen a saved Gmail draft by its draft id (e.g. from a scheduler toast). */
export async function openDraftById(accountId: string, draftId: string): Promise<void> {
  try {
    await openDraftVia(accountId, { draftId });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't open the draft: ${asCommandError(e).message}` });
  }
}

async function openDraftVia(accountId: string, ids: { draftId?: string; messageId?: string } | null): Promise<void> {
    const opened = ids ? await api.getDraft(accountId, ids) : null;
    if (!opened) {
      toast({ tone: "error", message: "Couldn't find that draft. It may have been sent or deleted." });
      return;
    }
    const d = opened.draft;
    const ctx: ComposeContext = {
      mode: d.replyToMessageId ? "reply" : "new",
      thread: { accountId: d.accountId, threadId: opened.threadId },
      messageId: opened.messageId,
      draftId: opened.draftId,
    };
    // Edits still open (or saving) in this session win over the server copy.
    const live = saverForDraftId(opened.draftId);
    if (live) {
      drafts.set(contextKey(ctx), { ...live.state, key: contextKey(ctx), ctx });
    } else {
      const split = splitQuote(d.bodyText);
      const body = split.body;
      // The quote's formatted HTML comes back from the HTML part (the text
      // part only has it as text).
      const quote = split.quote && d.bodyHtml ? { ...split.quote, html: quoteFromHtml(d.bodyHtml) } : split.quote;
      drafts.set(contextKey(ctx), {
        ...blankState(ctx, d.accountId),
        draftId: opened.draftId,
        draftAccountId: d.accountId,
        to: d.to,
        cc: d.cc,
        bcc: d.bcc,
        showCc: d.cc.length > 0,
        showBcc: d.bcc.length > 0,
        subject: d.subject,
        body,
        bodyHtml: d.bodyHtml,
        quote,
        replyingTo: quote && !quote.forward ? senderFromQuoteHeader(quote.header) : null,
        attachments: d.attachments ?? [],
        replyToThreadId: d.replyToThreadId,
        replyToMessageId: d.replyToMessageId,
      });
    }
    setUi({ overlay: "compose", composeContext: ctx });
}

// ---------------------------------------------------------------------------

const TITLES: Record<ComposeContext["mode"], string> = {
  new: "New message",
  reply: "Reply",
  replyAll: "Reply all",
  forward: "Forward",
};

function Composer({ ctx }: { ctx: ComposeContext }) {
  const tip = useKeyTip();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [st, setSt] = useState<EditorState | null>(() => drafts.get(contextKey(ctx)) ?? null);
  /** Nothing of this message was in memory when the composer opened (a fresh reply holds at most the signature). */
  const fresh = useRef(st === null);
  const [fromOpen, setFromOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [quoteOpen, setQuoteOpen] = useState(false);
  const body = useBodyHandle();
  const toRef = useRef<HTMLInputElement>(null);
  const ccRef = useRef<HTMLInputElement>(null);
  const bccRef = useRef<HTMLInputElement>(null);
  const subjectRef = useRef<HTMLInputElement>(null);
  const fromRef = useRef<HTMLButtonElement>(null);
  useDismiss(fromOpen, () => setFromOpen(false), [() => fromRef.current?.parentElement]);
  const [fromIdx, setFromIdx] = useState(0);
  const pendingText = useRef<Record<string, string>>({});
  // Opened with an instant reply or "Reply with AI" (the reply box, ⌘K).
  const [intent] = useState(() => takeComposeIntent());
  const [pickerOpen, setPickerOpen] = useState(false);
  const [writeOpen, setWriteOpen] = useState(!!intent?.ai);
  const [aiBusy, setAiBusy] = useState(false);
  const writerReady = useWriterReady();
  const aiAvailable = useAiAvailable();
  const instantSettings = useSetting("instantReplies");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [suggesting, setSuggesting] = useState(false);
  /** The {placeholders} the last send stopped for; sending again with the same ones goes ahead. */
  const placeholderWarned = useRef<string | null>(null);

  // Load accounts, then build (or restore) the draft.
  useEffect(() => {
    seedPeople();
    let alive = true;
    listOrderedAccounts()
      .then(async (accs) => {
        if (!alive) return;
        // In a profile, its accounts come first (and get ⌥1…); a new
        // message defaults to the filtered account, else the profile's first.
        accs = profileFirst(accs);
        setAccounts(accs);
        if (drafts.has(contextKey(ctx))) return;
        const base = blankState(ctx, pickAccount(accs, ctx.thread?.accountId, preferredFromAccount()));
        setSt(base);
        try {
          const filled = await prefill(base, accs);
          if (!alive) return;
          // Keep anything typed or attached while the thread was loading (and
          // the editor's document: it may already hold the signature).
          setSt((cur) =>
            cur && cur.touched
              ? { ...filled, body: cur.body, bodyDoc: cur.bodyDoc, attachments: withFiles(cur.attachments, filled.attachments), to: cur.to.length ? cur.to : filled.to, subject: cur.subject || filled.subject, touched: true }
              : cur
                ? { ...filled, body: cur.body, bodyDoc: cur.bodyDoc, attachments: withFiles(cur.attachments, filled.attachments) }
                : filled,
          );
        } catch (e) {
          if (alive) setError(`Couldn't load the original message: ${asCommandError(e).message}`);
        }
      })
      .catch((e) => alive && setError(`Couldn't load accounts: ${asCommandError(e).message}`));
    return () => {
      alive = false;
    };
  }, [ctx]);

  // The original's formatted body and a forward's files come from the
  // backend after the composer is up (downloading headers-only mail first).
  // Until that's done — or the user explicitly goes on without it — the
  // message can't be sent, so a forward never leaves without its files.
  const origLoad = st?.original ?? null;
  const [origStatus, setOrigStatus] = useState<OriginalStatus>(null);
  const [origTry, setOrigTry] = useState(0);
  useEffect(() => {
    if (!origLoad) {
      setOrigStatus(null);
      return;
    }
    let alive = true;
    setOrigStatus({ kind: "loading", downloading: origLoad.pending });
    loadOriginal(origLoad)
      .then((apply) => {
        if (!alive) return;
        setSt((s) => (s && s.original === origLoad ? apply(s) : s));
        setOrigStatus(null);
      })
      .catch((e) => alive && setOrigStatus({ kind: "error", message: asCommandError(e).message }));
    return () => {
      alive = false;
    };
  }, [origLoad, origTry]);
  /** Go on without the original's files and formatting (after a failed download). */
  const skipOriginal = () => setSt((s) => (s ? { ...s, original: null, touched: true } : s));
  const earlierOn = !!st && st.earlier.length > 0 && hasAllFiles(st.attachments, st.earlier);
  const setEarlier = (on: boolean) =>
    setSt((s) => (s ? { ...s, attachments: on ? withFiles(s.attachments, s.earlier) : withoutFiles(s.attachments, s.earlier), touched: true } : s));

  // Initial focus: recipients for a new message / forward, the body for replies.
  const focused = useRef(false);
  useEffect(() => {
    if (!st || focused.current) return;
    focused.current = true;
    requestAnimationFrame(() => {
      // Write with AI opened with it: its field keeps the focus.
      if (intent?.ai) return;
      // An instant reply went in: the caret goes after it, ready for ⌘↵.
      if (intent?.text && fresh.current) body.focus("end");
      else if (ctx.mode === "reply" || ctx.mode === "replyAll" || st.to.length) body.focus("start");
      else toRef.current?.focus();
    });
  }, [st, ctx.mode]);

  // Opened from an instant reply in the reply box or ⌘K: that text is the
  // message (a reply already in progress is kept as it is).
  const intentApplied = useRef(false);
  useEffect(() => {
    if (!st || intentApplied.current || !intent?.text) return;
    intentApplied.current = true;
    if (fresh.current) body.setWriting(intent.text);
    else toast({ message: "You were already writing this reply, so it's kept as it was" });
  }, [st, intent, body]);

  // Autosave: every edit goes to this draft's saver (debounced), and closing
  // saves right away. The in-memory copy covers the time until the save lands
  // (or forever, if the save keeps failing offline).
  const [saver, setSaver] = useState<DraftSaver | null>(() => (st ? saverFor(st) : null));
  useEffect(() => {
    if (st && !saver) setSaver(saverFor(st));
  }, [st, saver]);
  useEffect(() => {
    if (st && saver && st.touched) saver.update(st);
  }, [st, saver]);
  const status = useSaveStatus(saver);
  const snippets = useSnippets();
  const undoSeconds = useSetting("undoSendSeconds");

  const stRef = useRef(st);
  stRef.current = st;
  const saverRef = useRef(saver);
  saverRef.current = saver;
  useEffect(
    () => () => {
      const cur = stRef.current;
      const sv = saverRef.current;
      if (!cur || !sv) return;
      if (cur.touched) drafts.set(cur.key, { ...cur, draftId: sv.draftId, draftAccountId: sv.state.draftAccountId });
      void sv.flush().then(() => {
        // A new message that reached Gmail now lives in Drafts; C starts fresh.
        if (cur.key === "new" && sv.draftId) {
          drafts.delete("new");
          rekeySaver(sv, `draft:${sv.draftId}`);
        }
      });
    },
    [],
  );

  const account = accounts.find((a) => a.id === st?.accountId);
  const allRecipients = useMemo(() => (st ? [...st.to, ...st.cc, ...st.bcc] : []), [st]);
  const update = (patch: Partial<EditorState>) => setSt((s) => (s ? { ...s, ...patch, touched: true } : s));

  const myName = account?.displayName || account?.email.split("@")[0] || "";
  const snippetContext = (): SnippetContext => ({
    to: stRef.current?.to ?? [],
    replyingTo: stRef.current?.replyingTo ?? null,
    myName,
  });

  /**
   * A snippet went in (";trigger" or ⌘;): count the use, and add what it
   * brings besides text: a subject when there's none, Cc and Bcc, files.
   */
  function snippetUsed(sn: Snippet) {
    recordSnippetUse(sn.id);
    setSt((s) => {
      if (!s) return s;
      const x = snippetExtras(sn, s, parseAddress);
      if (!x.subject && !x.cc.length && !x.bcc.length) return s;
      return {
        ...s,
        subject: x.subject ?? s.subject,
        cc: [...s.cc, ...x.cc],
        bcc: [...s.bcc, ...x.bcc],
        showCc: s.showCc || x.cc.length > 0,
        showBcc: s.showBcc || x.bcc.length > 0,
        touched: true,
      };
    });
    if (sn.attachments.length) void attachSnippetFiles(sn);
  }

  async function attachSnippetFiles(sn: Snippet) {
    const have = new Set((stRef.current?.attachments ?? []).map((a) => `${a.kind === "file" ? a.filename : ""}`));
    const files = sn.attachments.filter((f) => !have.has(f.filename));
    const got = await Promise.all(
      files.map(async (f) => {
        try {
          return { kind: "file" as const, filename: f.filename, mimeType: f.mimeType, dataBase64: await api.readSnippetFile(f.id) };
        } catch (e) {
          toast({ tone: "error", message: `Couldn't attach “${f.filename}” from the snippet: ${asCommandError(e).message}` });
          return null;
        }
      }),
    );
    const added = got.filter((a): a is NonNullable<typeof a> => a !== null);
    if (!added.length) return;
    const cur = stRef.current;
    const total = [...(cur ? toDraft(cur).attachments ?? [] : []), ...added].reduce((n, a) => n + attachmentSize(a), 0);
    if (total > MAX_ATTACHMENT_BYTES) {
      setError(`The snippet's files would make ${(total / 1048576).toFixed(1)} MB of attachments; the limit is 25 MB.`);
      return;
    }
    setSt((s) => (s ? { ...s, attachments: [...s.attachments, ...added], touched: true } : s));
  }

  function pickSnippet(sn: Snippet) {
    setPickerOpen(false);
    const { text, selStart, selEnd } = expandSnippet(sn.body, snippetContext());
    body.insertExpanded(text, selStart, selEnd);
    snippetUsed(sn);
  }

  // Instant replies (reply composers): yours, and the model's suggestions
  // when they're on. Shown while nothing is written above the signature.
  const isReply = ctx.mode === "reply" || ctx.mode === "replyAll";
  const writing = st ? body.writingText() : "";
  // Not while Write with AI is open: its suggestion is about to fill the message.
  const choices = isReply && !writing.trim() && !writeOpen ? instantChoices(instantSettings, suggestions) : [];
  const suggestAsked = useRef(false);
  const threadForSuggest = ctx.thread ?? null;
  useEffect(() => {
    if (suggestAsked.current || !st || !threadForSuggest) return;
    if (!wantsSuggestions(instantSettings, ctx.mode, body.writingText(), aiAvailable)) return;
    suggestAsked.current = true;
    setSuggesting(true);
    api
      .suggestReplies(threadForSuggest.accountId, threadForSuggest.threadId)
      .then((r) => mounted.current && setSuggestions(r.replies))
      .catch(() => {
        // Suggestions are extra: without them the row shows your own one-liners.
      })
      .finally(() => mounted.current && setSuggesting(false));
  }, [st, threadForSuggest, instantSettings, ctx.mode, body, aiAvailable]);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  function pickInstant(c: InstantChoice) {
    body.setWriting(c.text);
    body.focus("end");
  }

  // The registered keys (commands.ts) reach this composer through the bus.
  const busRef = useRef({ pickSnippet: () => {}, write: () => {}, instant: (_n: number) => {}, send: () => {}, later: () => {} });
  busRef.current = {
    pickSnippet: () => setPickerOpen((o) => !o),
    write: () => {
      if (!writerReady) return;
      checkWriter();
      setWriteOpen((o) => !o);
    },
    instant: (n) => {
      const c = choices.find((x) => x.n === n);
      if (c) pickInstant(c);
    },
    send: () => send(),
    later: () => setMenu((m) => (m === "later" ? null : "later")),
  };
  useEffect(() => {
    setComposerBus({
      snippets: () => busRef.current.pickSnippet(),
      writeWithAi: () => busRef.current.write(),
      writerReady: () => writerReadyRef.current,
      instant: (n) => busRef.current.instant(n),
      send: () => busRef.current.send(),
      sendLater: () => busRef.current.later(),
    });
    return () => setComposerBus(null);
  }, []);
  const writerReadyRef = useRef(writerReady);
  writerReadyRef.current = writerReady;

  // Replies and forwards stay on the account the mail came to (Settings → Compose).
  const lockReplyAccount = useSetting("lockReplyAccount");
  const lockedTo = lockedAccount({ lockReplyAccount }, ctx);
  const lockedName = () => {
    const a = accounts.find((x) => x.id === lockedTo);
    return a ? accountName(a, accounts) : (lockedTo ?? "");
  };
  /** Switch From, unless this reply is locked to another account. */
  function pickFrom(accountId: string) {
    if (lockedTo && accountId !== lockedTo) {
      toast({ message: `Replies go from ${lockedName()}, the account this came to. Change it in Settings → Compose.` });
      return;
    }
    update({ accountId });
  }

  function close() {
    setUi({ overlay: null });
  }

  // In a compose window the composer is the window (app/windowShell.tsx
  // closes it when the composer closes); elsewhere it's an overlay with Pop out.
  const windowed = windowRoute.kind === "compose";
  const [popping, setPopping] = useState(false);

  // A compose window is titled with the subject (the tab and Window menu show it).
  const subjectForTitle = st ? composeTitle(st) : null;
  useEffect(() => {
    if (windowed && subjectForTitle) setThisWindowTitle(subjectForTitle);
  }, [windowed, subjectForTitle]);

  /**
   * Pop out: move this message into a window of its own, as it is. The draft
   * is saved first (so the new window continues it rather than making a
   * second one), then the whole editor state goes along as the window's
   * seed: recipients, body, files, From, the reply it answers, reminders.
   * This composer lets go of it only once the window exists.
   */
  async function popOut() {
    const cur = stRef.current;
    if (!cur || popping) return;
    // Addresses still being typed go along too.
    const next = { ...cur };
    for (const field of ["to", "cc", "bcc"] as const) {
      const a = parseAddress(pendingText.current[field] ?? "");
      if (a && !next[field].some((x) => x.email.toLowerCase() === a.email.toLowerCase())) next[field] = [...next[field], a];
    }
    const sv = saver ?? saverFor(next);
    setPopping(true);
    if (next.touched) sv.update(next);
    await sv.flush();
    const state = next.touched ? sv.current() : next;
    const seed: ComposeSeed = {
      v: 1,
      state: { ...state, draftId: sv.draftId, draftAccountId: sv.state.draftAccountId },
      unsaved: next.touched && !sv.isSaved(),
    };
    const { draftId, draftAccountId } = seed.state;
    try {
      await api.openWindow({
        kind: "compose",
        accountId: draftId ? (draftAccountId ?? seed.state.accountId) : null,
        draftId,
        seed,
        title: composeTitle(seed.state),
      });
    } catch (e) {
      setPopping(false);
      setError(`Couldn't open a window for this message: ${asCommandError(e).message}`);
      return;
    }
    // The window has it now: stop saving here without touching the draft.
    sv.stop();
    forgetSaver(sv);
    drafts.delete(cur.key);
    stRef.current = null;
    saverRef.current = null;
    setUi({ overlay: null });
  }

  // What the editor starts from, fixed at its first render (it owns the
  // document after that), and whether a fresh message gets a signature.
  const initialBody = useRef<{ doc: EditorState["bodyDoc"]; html: string | null; text: string; hasQuote: boolean } | null>(null);
  const signatureOnCreate = useRef<string | null>(null);
  if (st && !initialBody.current) {
    initialBody.current = { doc: st.bodyDoc, html: st.bodyHtml, text: st.body, hasQuote: !!st.quote };
    // (A "new" context with a messageId is a reopened draft; for replies it's the message answered.)
    const fresh = !st.draftId && !st.bodyDoc && !st.bodyHtml && !st.body && !ctx.draftId && !(ctx.mode === "new" && ctx.messageId);
    const s = currentSettings();
    signatureOnCreate.current = fresh && autoInserts(s, ctx.mode) ? (defaultSignature(s, st.accountId)?.id ?? null) : null;
  }
  // Switching From swaps in that account's signature.
  const sigAccount = useRef(st?.accountId ?? null);
  useEffect(() => {
    if (!st) return;
    if (sigAccount.current === null) sigAccount.current = st.accountId;
    if (sigAccount.current === st.accountId) return;
    sigAccount.current = st.accountId;
    body.accountChanged(defaultSignature(currentSettings(), st.accountId)?.id ?? null, autoInserts(currentSettings(), ctx.mode));
  }, [st?.accountId, st, body, ctx.mode]);

  const [discardArmed, setDiscardArmed] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [menu, setMenu] = useState<"later" | "remind" | null>(null);
  const [scheduling, setScheduling] = useState(false);
  const [scheduled, setScheduled] = useState<ScheduledSend | null>(null);
  // A reopened draft may already be scheduled: say so, and offer to cancel.
  const draftIdForSchedule = saver?.draftId ?? st?.draftId ?? null;
  const accountForSchedule = saver?.state.draftAccountId ?? st?.draftAccountId ?? st?.accountId ?? null;
  useEffect(() => {
    if (!draftIdForSchedule || !accountForSchedule) return;
    let alive = true;
    api
      .listScheduledSends(accountForSchedule)
      .then((list) => alive && setScheduled(list.find((x) => x.draftId === draftIdForSchedule) ?? null))
      .catch(() => alive && setScheduled(null));
    return () => {
      alive = false;
    };
  }, [draftIdForSchedule, accountForSchedule]);
  const fileRef = useRef<HTMLInputElement>(null);

  /**
   * Add files. Pasted or dropped ones (`inline`): images go in the text at
   * `at` (null: the caret) and are sent as inline parts; images over 10 MB
   * and other files attach. Picked ones (⌘⇧A) always attach.
   */
  async function attach(files: File[], opts: { inline?: boolean; at?: number | null } = {}) {
    const cur = stRef.current;
    if (!files.length || !cur) return;
    const split = opts.inline ? splitDropped(files) : { inline: [], tooBig: [], files };
    const ordered = [...split.inline, ...split.files];
    // What would go out now (images deleted from the text don't count).
    const { added, pictures, error: err } = await filesToAttachments(ordered, toDraft(cur).attachments ?? [], split.inline);
    if (err) setError(err);
    if (!added.length) return;
    for (const p of pictures) setInlineSrc(p.attrs.cid, p.dataUrl);
    setSt((s) => (s ? { ...s, attachments: [...s.attachments, ...added], touched: true } : s));
    if (pictures.length) body.insertImages(pictures.map((p) => p.attrs), opts.at ?? null);
    if (split.tooBig.length) {
      const names = split.tooBig.map((f) => `“${f.name || "image"}”`).join(", ");
      toast({ message: `${names} ${split.tooBig.length === 1 ? "is" : "are"} over 10 MB, so ${split.tooBig.length === 1 ? "it's" : "they're"} attached as a file instead of shown in the message` });
    }
  }

  /** "Send as attachment" on an inline image (the editor already took it out of the text). */
  const imageAsFile = (cid: string) => setSt((s) => (s ? { ...s, attachments: asFile(s.attachments, cid), touched: true } : s));

  // Inline images of a reopened draft show what the draft stored (read
  // through the backend once per image).
  const inlineKey = st ? st.attachments.map((a) => a.contentId ?? "").join(",") : "";
  useEffect(() => {
    const s = stRef.current;
    if (!s || !inlineKey) return;
    const shown = new Set([...docCids(s.bodyDoc), ...(s.bodyDoc ? [] : referencedCids(s.bodyHtml))]);
    for (const a of s.attachments) if (a.contentId && shown.has(a.contentId)) loadInlineSrc(a, s.draftAccountId ?? s.accountId);
  }, [inlineKey]);

  async function discard() {
    if (!st) return;
    const sv = saver;
    drafts.delete(st.key);
    stRef.current = null;
    saverRef.current = null;
    setUi({ overlay: null });
    if (!sv) return;
    forgetSaver(sv);
    try {
      await sv.discard();
      toast({ message: "Draft discarded" });
    } catch (e) {
      toast({ tone: "error", message: `Couldn't delete the draft from Gmail: ${asCommandError(e).message}` });
    }
  }

  /** Commit typed recipients and check the draft can go; null (with an error shown) if not. */
  function ready(): EditorState | null {
    if (!st) return null;
    // Commit whatever is still typed in a recipient box.
    const next = { ...st };
    for (const field of ["to", "cc", "bcc"] as const) {
      const raw = (pendingText.current[field] ?? "").trim();
      if (!raw) continue;
      const a = parseAddress(raw);
      if (!a) {
        setError(`“${raw}” isn't a valid email address.`);
        return null;
      }
      next[field] = [...next[field], a];
      pendingText.current[field] = "";
    }
    if (next.to.length + (next.showCc ? next.cc.length : 0) + (next.showBcc ? next.bcc.length : 0) === 0) {
      setError("Add at least one recipient.");
      toRef.current?.focus();
      return null;
    }
    if (!next.accountId) {
      setError("Add a Google account before sending.");
      return null;
    }
    if (next.original) {
      const what = next.original.forward ? "the original message and its attachments" : "the original message";
      setError(
        origStatus?.kind === "error"
          ? `Couldn't download ${what}. Retry, or choose to send without them.`
          : `Still loading ${what}. Send again in a moment.`,
      );
      return null;
    }
    if (lockedTo && next.accountId !== lockedTo) {
      setError(
        accounts.some((a) => a.id === lockedTo)
          ? `This came to ${lockedName()}, so the reply has to go from it too. Switch From back, or change it in Settings → Compose.`
          : `This came to ${lockedTo}, which isn't connected any more, so the reply can't go from another account. Change it in Settings → Compose.`,
      );
      return null;
    }
    const sending = toDraft(next).attachments ?? [];
    const total = sending.reduce((n, x) => n + attachmentSize(x), 0);
    if (total > MAX_ATTACHMENT_BYTES) {
      const what = sending.some((x) => x.contentId) ? "Attachments and images" : "Attachments";
      setError(`${what} total ${(total / 1048576).toFixed(1)} MB; the limit is 25 MB. Remove some or share a link.`);
      return null;
    }
    if (aiBusy) {
      setError("Accept or discard the suggested text first (↵ or Esc in Write with AI).");
      return null;
    }
    // A snippet's {placeholder} nobody filled in: say so once; sending again goes ahead.
    const left = unfilledPlaceholders(`${next.subject}\n${body.writingText() || next.body}`);
    const key = left.join(" ");
    if (left.length && placeholderWarned.current !== key) {
      placeholderWarned.current = key;
      setError(`${left.slice(0, 3).join(", ")} ${left.length === 1 ? "is" : "are"} still in the message. Tab jumps to ${left.length === 1 ? "it" : "them"}; send again to send it as it is.`);
      return null;
    }
    return next;
  }

  function send() {
    const next = ready();
    if (!next) return;
    stRef.current = null; // sent drafts aren't kept on close
    const sv = saver ?? saverFor(next);
    saverRef.current = null; // the send owns the saver now
    queueSend(next, sv);
    setUi({ overlay: null });
  }

  /** Send later: save the draft, then hand its id to the local scheduler. */
  async function scheduleAt(sendAt: number) {
    setMenu(null);
    const next = ready();
    if (!next || !saver) return;
    saver.update({ ...next, touched: true });
    setScheduling(true);
    try {
      await saver.flush();
      const draftId = saver.draftId;
      // A send later sends the draft exactly as saved, so the latest save
      // must have worked (an earlier version may lack files or edits).
      if (saver.status.kind === "error") throw new Error(saver.status.message);
      if (!draftId) throw new Error("the draft couldn't be saved");
      const sc = await api.scheduleSend({ accountId: next.accountId, draftId, sendAt, remindAfterMs: next.remindAfterMs });
      drafts.set(next.key, saver.current());
      stRef.current = null;
      setUi({ overlay: null });
      toast({
        message: `Scheduled for ${fmtWhen(sc.sendAt)} · ${RUNNING_NOTE}`,
        action: {
          label: "Undo",
          run: () => {
            api
              .cancelScheduledSend(sc.id)
              .then(() => setUi({ overlay: "compose", composeContext: next.ctx }))
              .catch((e) => toast({ tone: "error", message: `Couldn't cancel the schedule: ${asCommandError(e).message}` }));
          },
        },
        duration: 8000,
      });
    } catch (e) {
      setError(scheduleErrorText(e));
    } finally {
      setScheduling(false);
    }
  }

  async function unschedule() {
    if (!scheduled) return;
    try {
      await api.cancelScheduledSend(scheduled.id);
      setScheduled(null);
    } catch (e) {
      setError(`Couldn't cancel the schedule: ${asCommandError(e).message}`);
    }
  }

  function onKey(e: KeyboardEvent<HTMLElement>) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.shiftKey && e.key === "Enter") {
      e.preventDefault();
      setMenu((m) => (m === "later" ? null : "later"));
      return;
    }
    if (mod && e.key === "Enter") {
      e.preventDefault();
      send();
      return;
    }
    if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      if (discardArmed) setDiscardArmed(false);
      else if (fromOpen) setFromOpen(false);
      else close();
      return;
    }
    // ⌘⇧⌫ discards (asks first; a second ⌘⇧⌫ confirms).
    if (mod && e.shiftKey && (e.key === "Backspace" || e.key === "Delete")) {
      e.preventDefault();
      if (discardArmed) void discard();
      else setDiscardArmed(true);
      return;
    }
    // ⌥1–⌥9 pick the sending account (e.code: ⌥ changes e.key on macOS).
    if (e.altKey && !mod && /^Digit[1-9]$/.test(e.code)) {
      const a = accounts[Number(e.code.slice(5)) - 1];
      if (a) {
        e.preventDefault();
        pickFrom(a.id);
      }
      return;
    }
    // The From menu is keyboard-driven once open (⌘⇧O or a click).
    if (fromOpen && (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter")) {
      e.preventDefault();
      if (e.key === "Enter") {
        const a = accounts[fromIdx];
        if (a) pickFrom(a.id);
        setFromOpen(false);
        body.focus();
      } else {
        const n = accounts.length || 1;
        setFromIdx((i) => (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
      }
      return;
    }
    // Field jumps: ⌘⇧O From · ⌘⇧C Cc · ⌘⇧B Bcc · ⌘⇧S Subject · ⌘J body · ⌘S save now.
    const k = e.key.toLowerCase();
    if (mod && e.shiftKey && !e.altKey) {
      if (k === "o") {
        e.preventDefault();
        setFromIdx(Math.max(0, accounts.findIndex((a) => a.id === st?.accountId)));
        setFromOpen(true);
        fromRef.current?.focus();
        return;
      }
      if (k === "c" || k === "b") {
        e.preventDefault();
        if (k === "c") update({ showCc: true });
        else update({ showBcc: true });
        focusSoon(k === "c" ? ccRef : bccRef);
        return;
      }
      if (k === "a") {
        e.preventDefault();
        fileRef.current?.click();
        return;
      }
      if (k === "s") {
        e.preventDefault();
        subjectRef.current?.focus();
        subjectRef.current?.select();
        return;
      }
    }
    if (mod && !e.shiftKey && !e.altKey && k === "j") {
      e.preventDefault();
      body.focus("end");
      return;
    }
    if (mod && !e.shiftKey && !e.altKey && k === "s") {
      e.preventDefault();
      void saver?.flush();
    }
  }

  const title = ctx.draftId && ctx.mode === "new" ? "Draft" : TITLES[ctx.mode];
  const composer = (
    <section
      className={`composer panel cmp${windowed ? " cmp-windowed" : ""}${dropping ? " cmp-dropping" : ""}`}
      role={windowed ? "region" : "dialog"}
      aria-label={title}
      // Nothing changes while the message moves to its own window.
      inert={popping}
      aria-busy={popping}
      onKeyDown={onKey}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("Files")) return;
        e.preventDefault();
        setDropping(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(e) => {
        setDropping(false);
        // The text took it (images go where they were dropped).
        if (!e.dataTransfer.files.length || e.defaultPrevented) return;
        e.preventDefault();
        void attach([...e.dataTransfer.files], { inline: true, at: null });
      }}
    >
      {dropping && (
        <div className="cmp-drop" aria-hidden="true">
          <Icon name="clip" size="sm" />
          Drop to add: images go in the message, other files attach
        </div>
      )}
      <header className="cmp-head" data-tauri-drag-region={windowed ? "" : undefined}>
        <span className="cmp-title">{title}</span>
        <span className="grow" />
        {!windowed && (
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => void popOut()}
            disabled={!st || popping}
            title="Pop out: move this message to a window of its own"
            aria-label="Pop out into a new window"
          >
            <Icon name="popout" size="xs" />
            {popping ? "Opening…" : "Pop out"}
          </button>
        )}
        <button className="btn btn-ghost btn-sm" onClick={close} title={windowed ? "Close the window and keep the draft" : "Close and keep the draft"}>
          <Icon name="x" size="xs" />
          <span className="kbd">Esc</span>
        </button>
      </header>

      {!st ? (
        <div className="cmp-loading">Loading…</div>
      ) : (
        <>
          <div className="field">
            <span className="f-label">From</span>
            <div className="cmp-from">
              <button
                ref={fromRef}
                className={`from-select t-${accountTone(account?.color)}`}
                onClick={() => {
                  setFromIdx(Math.max(0, accounts.findIndex((a) => a.id === st.accountId)));
                  setFromOpen((o) => !o);
                }}
                title={tip("From", "⌘⇧O")}
                aria-haspopup="listbox"
                aria-expanded={fromOpen}
              >
                {account && <AccountAvatar account={account} accounts={accounts} />}
                <i className="dot" />
                <span className="emph">{account ? account.nickname || account.displayName || account.email : "No account"}</span>
                {(account?.nickname || account?.displayName) && <span className="faint">{account.email}</span>}
                <Icon name="down" size="xs" className="faint" />
              </button>
              {fromOpen && (
                <div className="panel menu cmp-from-menu" role="listbox">
                  {accounts.map((a, i) => (
                    <div
                      key={a.id}
                      role="option"
                      aria-selected={a.id === st.accountId}
                      className={`menu-item${i === fromIdx ? " active" : ""}`}
                      onMouseMove={() => setFromIdx(i)}
                      onMouseDown={(e) => e.preventDefault()}
                      aria-disabled={lockedTo !== null && a.id !== lockedTo}
                      title={lockedTo && a.id !== lockedTo ? `Locked: replies go from ${lockedName()}` : undefined}
                      onClick={() => {
                        pickFrom(a.id);
                        setFromOpen(false);
                      }}
                    >
                      <AccountAvatar account={a} accounts={accounts} />
                      <i className={`dot t-${accountTone(a.color)}`} />
                      <span className="emph">{accountName(a, accounts)}</span>
                      <span className="faint truncate">{a.email}</span>
                      {a.id === st.accountId && <Icon name="check" size="xs" className="accent-ico" />}
                      {lockedTo && a.id !== lockedTo ? <Icon name="lock" size="xs" className="faint" /> : i < 9 && <span className="kbd">⌥{i + 1}</span>}
                    </div>
                  ))}
                </div>
              )}
            </div>
            <span className="grow" />
            {lockedTo ? (
              <span className="f-hint cmp-from-lock" title="Replies and forwards go from the account the mail came to. Change it in Settings → Compose.">
                <Icon name="lock" size="xs" />
                Replying as {lockedName()}
              </span>
            ) : accounts.length > 1 && (
              <span className="f-hint">
                Switch
                <span className="kbd-group">
                  {accounts.slice(0, 9).map((_, i) => (
                    <span key={i} className="kbd">
                      ⌥{i + 1}
                    </span>
                  ))}
                </span>
              </span>
            )}
          </div>

          <RecipientField
            label="To"
            all={allRecipients}
            list={st.to}
            onChange={(to) => update({ to })}
            inputRef={toRef}
            onText={(t) => {
              pendingText.current.to = t;
            }}
            right={
              <span className="f-links">
                {!st.showCc && (
                  <a onClick={() => (update({ showCc: true }), focusSoon(ccRef))} title={tip("Cc", "⌘⇧C")}>
                    Cc
                  </a>
                )}
                {!st.showBcc && (
                  <a onClick={() => (update({ showBcc: true }), focusSoon(bccRef))} title={tip("Bcc", "⌘⇧B")}>
                    Bcc
                  </a>
                )}
              </span>
            }
          />
          {st.showCc && <RecipientField label="Cc" all={allRecipients} inputRef={ccRef} list={st.cc} onChange={(cc) => update({ cc })} onText={(t) => {
                pendingText.current.cc = t;
              }} />}
          {st.showBcc && <RecipientField label="Bcc" all={allRecipients} inputRef={bccRef} list={st.bcc} onChange={(bcc) => update({ bcc })} onText={(t) => {
                pendingText.current.bcc = t;
              }} />}

          <div className="field">
            <span className="f-label">Subject</span>
            <input
              ref={subjectRef}
              className="subject-input cmp-input"
              value={st.subject}
              onChange={(e) => update({ subject: e.target.value })}
              placeholder="Subject"
              aria-label="Subject"
            />
          </div>

          {scheduled && (
            <div className="cmp-scheduled" role="status">
              <Icon name="clock" size="xs" />
              <span className="cmp-scheduled-text truncate" title={`${RUNNING_NOTE}. Edits you make now are included.`}>
                Scheduled for <span className="emph">{fmtWhen(scheduled.sendAt)}</span>
                <span className="faint">
                  {scheduled.lastError ? ` · last try failed: ${scheduled.lastError}, retrying` : ` · ${RUNNING_NOTE}`}
                </span>
              </span>
              <span className="grow" />
              <button className="btn btn-ghost btn-sm" onClick={() => setMenu("later")}>
                Reschedule
              </button>
              <button className="btn btn-ghost btn-sm" onClick={() => void unschedule()}>
                Don't send later
              </button>
            </div>
          )}

          {choices.length > 0 || (suggesting && isReply && !writing.trim() && !writeOpen) ? (
            <InstantRow choices={choices} suggesting={suggesting && isReply && !writing.trim() && !writeOpen} onPick={pickInstant} />
          ) : null}

          {pickerOpen && (
            <div className="cmp-picker-anchor">
              <SnippetPicker
                snippets={snippets}
                context={snippetContext()}
                onPick={pickSnippet}
                onClose={() => {
                  setPickerOpen(false);
                  body.focus();
                }}
              />
            </div>
          )}

          <Suspense fallback={<div className="cmp-body cmp-body-live"><div className="cmp-rich-loading">Write your message…</div></div>}>
          <RichBody
            handle={body}
            initial={initialBody.current ?? { doc: st.bodyDoc, html: st.bodyHtml, text: st.body, hasQuote: !!st.quote }}
            signatureOnCreate={signatureOnCreate.current}
            onChange={(bodyDoc, text, byUser) => {
              if (byUser) update({ bodyDoc, body: text });
              else setSt((s) => (s ? { ...s, bodyDoc, body: text } : s));
            }}
            quote={st.quote}
            quoteOpen={quoteOpen}
            setQuoteOpen={setQuoteOpen}
            snippets={snippets}
            snippetContext={snippetContext}
            onSnippetUsed={snippetUsed}
            onPasteFiles={(files, at) => void attach(files, { inline: true, at })}
            onImageAsFile={imageAsFile}
          />
          </Suspense>

          {writeOpen && writerReady && (
            <WriteBar
              body={body}
              context={() => ({
                // The conversation's own account: its messages are read from there.
                accountId: (isReply || ctx.mode === "forward" ? ctx.thread?.accountId : null) ?? stRef.current?.accountId ?? st.accountId,
                threadId: isReply || ctx.mode === "forward" ? (ctx.thread?.threadId ?? null) : null,
                reply: isReply,
                subject: stRef.current?.subject ?? "",
                recipients: (stRef.current?.to ?? []).map((a) => displayName(a)),
                myName,
              })}
              onBusy={setAiBusy}
              onClose={() => {
                setWriteOpen(false);
                body.focus();
              }}
            />
          )}

          <AttachmentList list={st.attachments} onRemove={(i) => update({ attachments: st.attachments.filter((_, j) => j !== i) })} />
          <OriginalFiles
            status={origStatus}
            forward={ctx.mode === "forward"}
            earlier={st.earlier}
            earlierOn={earlierOn}
            onEarlier={setEarlier}
            onRetry={() => setOrigTry((n) => n + 1)}
            onSkip={skipOriginal}
          />

          {error && (
            <div className="cmp-error" role="alert">
              <Icon name="info" size="xs" />
              {error}
              <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setError(null)} aria-label="Dismiss">
                <Icon name="x" size="2xs" />
              </button>
            </div>
          )}

          <div className="cmp-tools">
            <button className="btn btn-primary btn-sm" onClick={send} disabled={!account}>
              Send<span className="kbd">⌘↵</span>
            </button>
            <span className="cmp-menu-anchor">
              <button
                className={`btn btn-secondary btn-sm${menu === "later" ? " is-open" : ""}`}
                onClick={() => setMenu((m) => (m === "later" ? null : "later"))}
                disabled={!account || scheduling}
                title={`${tip("Send later", "⌘⇧↵")} · ${RUNNING_NOTE}`}
              >
                <Icon name="clock" size="xs" />
                {scheduling ? "Scheduling…" : "Send later"}
                <span className="kbd">⌘⇧↵</span>
              </button>
              {menu === "later" && (
                <SendLaterMenu scheduled={scheduled} onPick={(ms) => void scheduleAt(ms)} onClose={() => setMenu(null)} />
              )}
            </span>
            <span className="cmp-menu-anchor">
              <button
                className={`btn btn-ghost btn-sm${st.remindAfterMs ? " cmp-remind-on" : ""}${menu === "remind" ? " is-open" : ""}`}
                onClick={() => setMenu((m) => (m === "remind" ? null : "remind"))}
                title="Remind me if nobody replies"
              >
                <Icon name={st.remindAfterMs ? "bell" : "belloff"} size="xs" />
                {st.remindAfterMs ? `Remind · ${remindLabel(st.remindAfterMs)}` : "Remind"}
              </button>
              {menu === "remind" && (
                <RemindMenu
                  value={st.remindAfterMs}
                  onPick={(ms) => {
                    update({ remindAfterMs: ms });
                    setMenu(null);
                    body.focus();
                  }}
                  onClose={() => setMenu(null)}
                />
              )}
            </span>
            <span className="tool-sep" />
            <button
              className={`btn btn-ghost btn-sm${pickerOpen ? " is-open" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setPickerOpen((o) => !o)}
              title={`${tip("Insert snippet", "⌘;")} — or type ; and a trigger`}
            >
              <Icon name="zap" size="xs" />
              Snippets<span className="kbd">⌘;</span>
            </button>
            {writerReady && (
              <button
                className={`btn btn-ghost btn-sm${writeOpen ? " is-open" : ""}`}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => busRef.current.write()}
                title={`${tip("Write with AI", "⌘⇧J")} · Apple Intelligence, on this Mac`}
              >
                <Icon name="sparkles" size="xs" />
                Write<span className="kbd">⌘⇧J</span>
              </button>
            )}
            <button className="btn btn-ghost btn-sm" onClick={() => fileRef.current?.click()} title={`${tip("Attach files", "⌘⇧A")} — or drop or paste them`}>
              <Icon name="clip" size="xs" />
              Attach<span className="kbd">⌘⇧A</span>
            </button>
            <input
              ref={fileRef}
              type="file"
              multiple
              hidden
              onChange={(e) => {
                const files = [...(e.target.files ?? [])];
                e.target.value = "";
                void attach(files);
              }}
            />
            <span className="grow" />
            {discardArmed ? (
              <span className="cmp-discard" role="alert">
                Discard this draft{st.draftId ? " and remove it from Gmail" : ""}?
                <button className="btn btn-sm cmp-discard-yes" onClick={() => void discard()}>
                  Discard <span className="kbd">⌘⇧⌫</span>
                </button>
                <button className="btn btn-ghost btn-sm" onClick={() => setDiscardArmed(false)}>
                  Keep <span className="kbd">Esc</span>
                </button>
              </span>
            ) : (
              <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setDiscardArmed(true)} title={tip("Discard draft", "⌘⇧⌫")}>
                <Icon name="trash" size="xs" />
              </button>
            )}
          </div>
          <div className="cmp-foot">
            {undoSeconds > 0 ? (
              <span title="Change it in Settings → Compose">
                <Icon name="undo" size="2xs" />
                Undo send for {undoSeconds} seconds <span className="kbd">Z</span>
              </span>
            ) : (
              <span title="Turn it on in Settings → Compose">
                <Icon name="undo" size="2xs" />
                Undo send is off
              </span>
            )}
            <span className="grow" />
            <SaveBadge status={status} />
            {account && (
              <span className={`t-${accountTone(account.color)}`}>
                <i className="dot dot-sm" />
                Sending as <span className="emph">{accountName(account, accounts)}</span>
              </span>
            )}
          </div>
        </>
      )}
    </section>
  );
  // A compose window: the composer fills it (no scrim; closing it closes the window).
  if (windowed) return <div className="cmp-window">{composer}</div>;
  return (
    <>
      <div className="scrim" onMouseDown={close} />
      <div className="overlay-host center cmp-host" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        {composer}
      </div>
    </>
  );
}

/** Focus a field after React has rendered it (Cc/Bcc appear on demand). */
function focusSoon(ref: React.RefObject<HTMLInputElement | HTMLTextAreaElement | null>, toEnd = false) {
  requestAnimationFrame(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    if (toEnd) el.setSelectionRange(el.value.length, el.value.length);
  });
}

function SaveBadge({ status }: { status: SaveStatus }) {
  switch (status.kind) {
    case "idle":
      return null;
    case "saving":
      return <span className="cmp-save">Saving…</span>;
    case "saved":
      return (
        <span className="cmp-save" title={`Saved to Gmail Drafts at ${new Date(status.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`}>
          <Icon name="check" size="2xs" />
          Saved
        </span>
      );
    case "error":
      return (
        <span className="cmp-save cmp-save-err" title={status.message}>
          {status.offline ? "Offline · kept here, will retry" : "Not saved · retrying"}
        </span>
      );
  }
}

// ---------------------------------------------------------------------------
// Recipients
// ---------------------------------------------------------------------------

function RecipientField({
  label,
  list,
  all,
  onChange,
  onText,
  inputRef,
  right,
}: {
  label: string;
  list: Address[];
  /** Every recipient in To/Cc/Bcc, to spot two people with the same name. */
  all: Address[];
  onChange: (l: Address[]) => void;
  onText: (t: string) => void;
  inputRef?: React.RefObject<HTMLInputElement | null>;
  right?: ReactNode;
}) {
  const [text, setText] = useState("");
  const [idx, setIdx] = useState(0);
  const [open, setOpen] = useState(false);
  const people = usePeople();
  const exclude = useMemo(() => new Set(list.map((a) => a.email.toLowerCase())), [list]);
  const sugg = useMemo(() => (text.trim() ? matchPeople(people, text, exclude, 6) : []), [people, text, exclude]);
  const show = open && sugg.length > 0;
  const localRef = useRef<HTMLInputElement>(null);
  const ref = inputRef ?? localRef;
  const chipEls = useRef<Array<HTMLSpanElement | null>>([]);
  const clashes = useMemo(() => clashingNames(all), [all]);

  function focusChip(i: number) {
    if (i < 0) i = 0;
    if (i >= list.length) return ref.current?.focus();
    chipEls.current[i]?.focus();
  }

  function removeAt(i: number) {
    onChange(list.filter((_, j) => j !== i));
    // Stay on the keyboard: the chip before it, else the typing box.
    requestAnimationFrame(() => (i > 0 ? chipEls.current[i - 1]?.focus() : ref.current?.focus()));
  }

  useEffect(() => {
    onText(text);
  }, [text, onText]);
  useLayoutEffect(() => setIdx(0), [text]);

  function add(a: Address) {
    onChange([...list, a]);
    setText("");
  }

  function commitTyped(): boolean {
    const a = parseAddress(text);
    if (a && !exclude.has(a.email.toLowerCase())) {
      add(a);
      return true;
    }
    return false;
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (show && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setIdx((i) => (i + (e.key === "ArrowDown" ? 1 : sugg.length - 1)) % sugg.length);
      return;
    }
    if ((e.key === "Enter" && !e.metaKey && !e.ctrlKey) || (e.key === "Tab" && text.trim()) || e.key === "," || e.key === ";") {
      if (show) {
        e.preventDefault();
        add(sugg[idx]);
        return;
      }
      if (text.trim()) {
        if (commitTyped()) e.preventDefault();
        else if (e.key !== "Tab") e.preventDefault();
      }
      return;
    }
    // ⌫ in an empty box selects the last chip (a second ⌫ removes it); ← too.
    if ((e.key === "Backspace" || e.key === "ArrowLeft") && !text && list.length) {
      e.preventDefault();
      focusChip(list.length - 1);
      return;
    }
    if (e.key === "Escape" && show) {
      e.preventDefault(); // close the dropdown, not the composer
      setOpen(false);
    }
  }

  return (
    <div className="field cmp-rcpts" onMouseDown={(e) => e.target === e.currentTarget && (e.preventDefault(), ref.current?.focus())}>
      <span className="f-label">{label}</span>
      <div className="cmp-rcpt-wrap">
        {list.map((a, i) => (
          <RecipientChip
            key={`${a.email}-${i}`}
            address={a}
            clash={!!a.name && clashes.has(a.name.trim().toLowerCase())}
            chipRef={(el) => {
              chipEls.current[i] = el;
            }}
            onRemove={() => removeAt(i)}
            onReplace={(next) => onChange(list.map((x, j) => (j === i ? next : x)))}
            onFocusInput={() => ref.current?.focus()}
            onKeyNav={(d) => focusChip(i + d)}
          />
        ))}
        <div className="cmp-rcpt-input">
          <input
            ref={ref}
            className="cmp-input"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setOpen(true);
            }}
            onKeyDown={onKey}
            onFocus={() => {
              seedPeople(); // no-op once people are known
              setOpen(true);
            }}
            onBlur={() => {
              setOpen(false);
              commitTyped();
            }}
            aria-label={label}
            spellCheck={false}
            autoComplete="off"
          />
          {show && (
            <div className="panel menu cmp-ac" role="listbox">
              {sugg.map((a, i) => (
                <div
                  key={a.email}
                  role="option"
                  aria-selected={i === idx}
                  className={`menu-item${i === idx ? " active" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    add(a);
                  }}
                >
                  <Avatar person={a} size="xs" />
                  <span className="emph">{displayName(a)}</span>
                  <span className="faint truncate">{a.email}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
      {right}
    </div>
  );
}
