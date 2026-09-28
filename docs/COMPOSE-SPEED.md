# Compose and reply speed

Four features for writing mail faster:

- **Snippets** with variables, Cc/Bcc, a subject and files.
- **Instant replies**, your own one-liners plus optional on-device suggestions.
- **Write with AI** in the composer (Apple's on-device model).
- **Undo send and Send later**, checked and filled in.

Each one is switched on or off in Settings, and none of them needs a server or a cloud model.

This document says what each feature does, which keys reach it, and how other mail apps do the same thing, with sources. The research date is 2026-09-27. The Apple Foundation Models bridge these features share is described in [SUMMARIES.md](SUMMARIES.md). The earlier background research is [research/15](../research/15-fast-reply-and-focus-mode.md) §1.

**How the sources were checked.** Superhuman's help center (`help.superhuman.com`) answers every automated fetch with 403. Facts marked *(excerpt)* come from search-result excerpts of those articles. Everything else is from pages fetched directly: Superhuman's changelog, blog and shortcut sheet, and Google's, Readdle's, Mimestream's and Apple's own documentation. Apple's developer pages were read through their documentation JSON. Penguin copies none of any product's names or branding, only the mechanics.

## At a glance

| | Keys | Setting |
|---|---|---|
| Insert a snippet | `;trigger` while writing, or **⌘;** for the picker | Settings → Compose → Snippets |
| Instant reply | **⌃1–⌃9** in a reply (before anything is written), a click on the chip, a chip in the thread's reply box, or ⌘K "Reply “…”" | Settings → Compose → Instant replies (on) |
| Suggested replies (AI) | The same row, marked ✦ | Settings → AI → Suggested replies (off) |
| Write with AI | **⌘⇧J** in the composer; **1–4** for Shorter, Friendlier, More formal and Fix spelling & grammar; **↵** accepts and **Esc** discards. ⌘K "Reply with AI…" starts a reply with it open | Settings → AI → Write with AI (on) |
| Send | **⌘↵** | — |
| Undo send | **Z** (or the toast's Undo) during the window | Settings → Compose → Undo send: Off, 5, 10 (default), 20 or 30 s |
| Send later | **⌘⇧↵**, then **1–5** for a preset, or type a time ("fri 3pm") | Settings → Compose → Morning send time (6–10 AM, default 8) |

Every one of these keys is in the **?** sheet under Compose. The composer's keys are listed in the shortcut registry (`features/compose/commands.ts`), and while the composer is open they reach it through a small bus, not through the app's list keys.

**Collisions checked.**
- **⌘;** was free.
- **⌘⇧J** was free. Superhuman uses ⌘J for its AI, but Penguin's ⌘J already jumps to the message body.
- **⌃1–⌃9** switch profiles in the mail list. Inside the composer, where profile switching can't happen, the same keys pick an instant reply. ⌥1–⌥9 stay the From account.
- **⌘⇧L**, Superhuman's send-later key, is Star in Penguin's menu bar, so Send later stays on ⌘⇧↵.
- The palette key ⌘K is the link editor inside the composer. So ⌘K lists the commands that *start* writing: "Reply with AI…", "Reply “Sounds good, thanks!”", "Manage snippets…" and "Manage instant replies…".

## 1. Snippets

### What other apps do

**Superhuman**
- ⌘; opens the snippet picker, and `;` inserts inline ([shortcut sheet](https://download.superhuman.com/Superhuman%20Keyboard%20Shortcuts.pdf)).
- Built-in variables `{first_name}`, `{last_name}`, `{full_name}` ([changelog](https://new.superhuman.com/variables-in-snippets-198344)), plus `{sender_first_name}`.
- Any other `{phrase}` becomes a placeholder, and the app warns before sending one unfilled (same changelog).
- A snippet can carry recipients, Cc, Bcc, a subject and attachments ([blog](https://blog.superhuman.com/snippets/)).

**Gmail**
- Templates are off by default and turned on under Settings > Advanced ([support](https://support.google.com/mail/answer/14864208?hl=en)).
- Inserted from the ⋮ menu. No variables, no keyboard shortcut.

**Spark**
- Templates have To/Cc/Bcc, a subject, a body and files.
- Placeholders for the recipient's and your own first, last and full name, plus custom ones filled in by hand ([support](https://support.readdle.com/spark/sending-emails/use-email-templates)).
- Names fail when To holds no name (same page).

**Mimestream**
- Templates with a subject, To/Cc/Bcc, `{{ recipient.first_name }}` variables and custom variables that ask for a value.
- Inserted with ⌘/ ([user guide](https://mimestream.com/help/user-guide/templates), [1.0 notes](https://mimestream.com/blog/whats-new-in-1.0)).
- It can't use Gmail's own templates, because the Gmail API doesn't expose them (same guide).

**Apple Mail** has no snippet or template feature that we could find.

### What Penguin does

- **Triggers.** `;trigger` completes inline (as before). **⌘;** opens a picker over the message that searches trigger, name and text, most-used first. It previews the text with the variables filled in from this message, and shows the subject, Cc, Bcc and files the snippet brings.
- **Variables** (`features/compose/snippets.ts`), from the first To recipient:
  - `{first_name}`, `{last_name}`, `{full_name}`. "Raman, Priya" reads last-name-first.
  - `{company}`, from the recipient's address domain: `priya@mail.northwind.example` → Northwind, `acme.co.uk` → Acme. Personal mail domains such as Gmail, Outlook and iCloud give nothing.
  - `{sender_name}` (the person you're replying to), `{my_name}`, `{my_first_name}`, `{date}`.
  - `{cursor}`, where the caret lands.
  - A variable with no value (no recipient yet) stays as a placeholder. Tab jumps between placeholders.
  - **Send asks first** while a `{placeholder}` is left in the subject or text: "{day} is still in the message… send again to send it as it is". A second ⌘↵ sends.
- **Extras.**
  - A snippet can set a **subject**, used only when the message has none.
  - It can add **Cc** and **Bcc**. Addresses already on the message aren't added twice, and the rows open by themselves.
  - It can attach up to 10 **files**. Files are kept on this Mac, content-addressed at `<data dir>/snippets/<sha256>` (`save_snippet_file` / `read_snippet_file`). They're read when the snippet goes in and attached like picked files, within the 25 MB limit. Files no snippet uses are removed at launch once they're a day old.
- **Settings → Compose → Snippets** edits all of it. The variables are one-click chips, and "Subject, Cc, Bcc, files" opens the extras. `Snippet` in `settings.rs` / `types.ts` gained `subject`, `cc`, `bcc` and `attachments`. They're normalized on write (≤ 200-char subject, ≤ 20 addresses each, ≤ 10 files with a valid sha256 id), and snippets saved before this load with none.

## 2. Instant replies

### What other apps do

**Superhuman Instant Reply**
- Three AI-suggested replies. "Press the tab to switch between replies and press enter to send one" ([TechCrunch](https://techcrunch.com/2024/02/27/superhuman-launches-an-ai-powered-instant-replies-feature/), [changelog](https://new.superhuman.com/instant-reply-286708)).
- They sit at the bottom of the latest message, and AI features are switched on as a whole *(excerpt)*.
- Cloud model.

**Gmail Smart Reply**
- "A few response options at the bottom of their screen that take the full content of the email thread into consideration" ([Workspace updates](https://workspaceupdates.googleblog.com/2024/09/contextual-smart-replies.html)).
- Server-side, and needs "Smart features" on.

**Spark Quick Replies**
- Your own canned replies (name, text, emoji), under Settings > General > Quick Replies.
- One click *sends immediately*, with 5 s to undo ([support](https://support.readdle.com/spark/tips-tricks/answer-emails-using-quick-replies)).
- Spark +AI can also suggest replies ([support](https://support.readdle.com/spark/tips-tricks/spark-ai)).

**Apple Mail Smart Reply**
- "Choose a suggested reply… Apple Intelligence drafts a reply in the email" ([Mac User Guide](https://support.apple.com/guide/mac-help/use-apple-intelligence-in-mail-mchlb2dbea8f/15.0/mac/15.0)).
- Apple's developer note says a long-form app should use a suggestion "to generate a long-form response with your own model" rather than drop it in as is ([UIKit](https://developer.apple.com/documentation/UIKit/adopting-smart-reply-in-your-messaging-or-email-app)). That API is UIKit-only.

### What Penguin does

- **Your one-liners** (up to nine, three to start with) show as a row above the text in a reply composer while nothing is written above the signature.
  - **⌃1–⌃9** or a click puts one in as the message, caret after it.
  - **⌘↵** sends it, with the usual undo window.
  - Unlike Spark, a pick never sends by itself: one keystroke more, but no one-click accidents.
- **In the thread's reply box**, the same chips sit under "Write a reply…". A click opens the reply with that text in it.
- **In ⌘K** with a conversation selected: "Reply “Sounds good, thanks!”".
- **Suggested replies** (Settings → AI, off by default).
  - When a reply opens, Apple's on-device model suggests up to three short replies to the latest message. They're numbered after yours and marked ✦.
  - They're asked for once per reply, only when nothing is written and Apple Intelligence is available.
  - The backend caches them per thread version.
  - They're hidden when Apple Intelligence is unavailable, and your own one-liners always show.
- **Settings → Compose → Instant replies**: an on/off switch; add, edit in place, reorder and delete. `Settings.instantReplies {enabled, replies, aiSuggestions}`; one-liners are trimmed, deduped, ≤ 200 chars, ≤ 9.

## 3. Write with AI

### What other apps do

**Superhuman "Write with AI"**
- "When drafting a message, hit Cmd+J… type a prompt and hit Enter". Inside a draft, it edits with suggestions or "Describe how to edit the text" *(excerpt)*.
- Cloud.

**Gmail "Help me write"**
- Prompt, then Create. Refine with "Formalize", "Friendly", "Shorten" or custom instructions ([support](https://support.google.com/mail/answer/13955415?hl=en)).
- Gemini, cloud.

**Spark +AI**
- Generate a draft, then Proofread, Rephrase, Expand/Shorten, friendly or formal.
- The result shows in a panel with Adjust and Insert.
- Azure OpenAI ([support](https://support.readdle.com/spark/tips-tricks/spark-ai)).

**Apple Writing Tools** (Mail and system-wide)
- Proofread, Rewrite, Friendly, Professional, Concise, and "Describe a change". Rewrites can be reverted ([Mac User Guide](https://support.apple.com/guide/mac-help/mchldcd6c260/mac)).
- Mimestream offers exactly this in its composer, and "prevents you from rewriting your signature or the quoted text" ([user guide](https://mimestream.com/help/user-guide/writing-tools)).

### What Penguin does

- **⌘⇧J** (or **Write** in the composer's tool row) opens a bar under the text. It works on:
  - **the selection**, when there is one. It's clipped to your own writing, so the signature is never rewritten, and the quote isn't in the editor at all;
  - otherwise, **everything written above the signature**;
  - with nothing written, it **drafts** from what you type. For a reply, the conversation's latest messages are the context.
- **Presets.** 1 Shorter · 2 Friendlier · 3 More formal · 4 Fix spelling & grammar (Gmail's and Apple's sets), or type an instruction. An instruction like "make it warmer" or "translate to Spanish" changes the text. Anything else drafts, using what's written as the starting point (`writePrompt.ts`).
- **In the text, as a suggestion** (`editor/aiSuggest.ts`).
  - The model's words stream in *in place*: the text it would replace is struck through, and the new text follows in the accent color with a caret.
  - The editor is read-only meanwhile, and nothing in the document changes until you choose.
  - **↵ Accept** puts it in as one undoable edit (⌘Z brings the old text back). **Esc Discard** returns to the prompt, and **Try again** asks again.
  - Esc while it's writing stops it.
  - Send is held while a suggestion is showing ("Accept or discard the suggested text first").
- **Degrades gracefully.** The Write button, ⌘⇧J and ⌘K "Reply with AI…" only exist when the setting is on *and* the Mac can run Apple Intelligence. Availability is checked like summaries: at launch, when Settings → AI opens, and again when the bar opens. Settings → AI says why when it can't run. Errors show in the bar with Try again.
- **Settings → AI → Write with AI**: on by default, because it only runs when asked and only on this Mac. It replaces the old placeholder "Draft with AI" row.

The model side (`src-tauri/src/writing/`, `penguin-core/src/writing.rs`, the Swift bridge) is described in **The on-device model** below.

## 4. Undo send and Send later

### What other apps do

| | Undo send | Send later |
|---|---|---|
| Superhuman | Fixed 10 s, `Z`, can't be extended *(excerpt)* | ⌘⇧L. Natural language ("tomorrow morning", time zones); won't send if they reply first ([changelog](https://new.superhuman.com/send-later-with-peace-of-mind-134800)) |
| Gmail | 5, 10, 20 or 30 s ([support](https://support.google.com/mail/answer/2819488?hl=en)) | Tomorrow morning 8 AM, Tomorrow afternoon 1 PM, Monday morning 8 AM, Pick date & time. Server-side ([support](https://support.google.com/mail/answer/9214606?hl=en)) |
| Spark | Selectable, default 5 s ([help](https://sparkmailapp.com/help/sending-emails/customize-undo-send-timer)) | Customizable presets; Spark's servers hold and send them ([support](https://support.readdle.com/spark/sending-emails/schedule-an-email-to-send-later)) |
| Mimestream | 5, 10, 20 or 30 s ([guide](https://mimestream.com/help/user-guide/composing-settings)) | None (roadmap) |
| Apple Mail | Off, 10, 20 or 30 s; default 10 ([guide](https://support.apple.com/guide/mail/send-email-messages-mlhlp1098/mac)) | 9:00 PM tonight, 8:00 AM tomorrow, Send Later… (same guide) |

### What Penguin had, and what changed

Both already existed:
- **Undo send.** `send.ts` counts down in a toast with Undo, and **Z** reopens the draft.
- **Send later.** ⌘⇧↵ runs the local scheduler (`penguin-provider` outbox), with presets and a date picker. It "sends when Penguin is running", since the Gmail API has no scheduled send (research/15 §1).

The gaps, now filled:
- **Undo send can be turned off**, as in Apple Mail: `undoSendSeconds` takes 0, 5, 10, 20 or 30. At 0 the message goes as soon as the draft save settles, and the composer's footer says "Undo send is off".
- **Presets follow Gmail's**: In 1 hour · This afternoon (5 PM, until 4 PM) · Tomorrow morning · Tomorrow afternoon (1 PM) · Monday morning.
  - The morning ones use **Settings → Compose → Morning send time** (`sendLaterHour`, 6–10 AM shown, 5–11 accepted, default 8).
  - **1–5** pick a preset.
- **Type a time.** In the menu, start typing ("tomorrow 9am", "fri 3pm", "oct 3 10:30", "in 2 hours", "tonight") and it reads the time back as you type (`when.ts`); ↵ schedules. The date-and-time picker is still there.
- Send, Send later and Undo are listed in the **?** sheet.

## The on-device model

Write with AI and suggested replies use the same bridge as thread summaries ([SUMMARIES.md](SUMMARIES.md)): Apple's Foundation Models framework on macOS 26+ with Apple Intelligence, and one generation at a time. They share the summarizer's gate, so a summary and a rewrite never compete for the Neural Engine. Nothing leaves the Mac, and nothing is logged but ids, counts, timings and an outcome code.

- **Commands.**
  - `write_with_ai(request)` streams `penguin://write-progress {runId, text}` (snapshots, not deltas) and returns the final text. `cancel_write(runId)` stops it.
  - `prewarm_writer` loads the model when the bar opens. Apple: prewarm when there's a second or more before the request ([docs](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/prewarm(promptprefix:))).
  - `suggest_replies(accountId, threadId)` returns up to three replies, non-streaming, as Apple advises for requests the user isn't watching ([streamResponse](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/streamresponse(to:options:))).
- **Prompts** (`penguin-core/src/writing.rs`, unit-tested on Linux).
  - The fixed rules are in the *instructions*. Your instruction, your text and the conversation go only in the *prompt*, in labeled sections, and the instructions say to ignore instructions inside emails. This is the same injection stance as summaries ([Apple: safety](https://developer.apple.com/documentation/foundationmodels/improving-the-safety-of-generative-model-output)).
  - Reply context is the conversation's authored text (quotes cut) through the summaries' `build_input`, newest messages that fit, oldest first.
  - The output is cleaned: no "Subject:" line, "Here's…" preamble or markdown.
- **Output.**
  - Rewrites and drafts are plain text (`String`). When the default guardrails refuse, they retry once with `.permissiveContentTransformations`, which relaxes only `String` output ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/guardrails/permissivecontenttransformations)).
  - Suggestions use guided generation: a new `@Generable ReplyOptions { replies: [String] }` in the Swift bridge, selected by a `schema: "replies"` field on the bridge request, alongside the summaries' `ThreadDigest`. Plain-text numbered lines are the fallback.
- **The context window** is 4,096 tokens per session ([TN3193](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)). The writer keeps the conversation to about half of it, and the rest is for the instructions, your text and the answer.

## Testing

- **UI** (`tests/composeSpeed.test.ts`): snippet variables, names and companies, unfilled placeholders, matching, extras; instant-reply choices, when suggestions are asked for, the compose intent; the Write bar's requests; the selection and writing targets on the real editor schema; send-later presets and typed times.
- **Settings** (`settings.rs`): undo 0, the morning hour, snippet extras normalized, instant replies' defaults and limits, the wire format.
- **Model side**: see the writing module's tests (a scripted engine, as for summaries) and `penguin-core` `writing_tests.rs`.
- **Mock** (`npm run dev:mock`): `src/lib/mock/writing.ts` is a fake streaming writer.
  - `?aiReplies=1` turns suggestions on, `?instant=off` hides instant replies, and `?writeAi=off` turns Write with AI off.
  - `?ai=notEnabled|noDevice|notReady|old|refuse|slow` show the other states.
  - Seeded snippet `;pricing` has a subject, a Cc and a file.
- **macOS CI** (`.github/workflows/macos-check.yml`) builds the Swift bridge with the new schema on each Xcode it checks. Runners can't run Apple Intelligence (VMs), so what the model actually writes needs a real Mac.
