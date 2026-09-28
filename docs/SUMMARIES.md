# Thread summaries (on-device)

Penguin summarizes a conversation with **Apple's on-device foundation model**, through the Foundation Models framework. Nothing leaves the Mac, there is no API key, and nothing runs until you ask. This document covers what it does, why it's built this way, and the Apple sources each decision rests on. Research date: 2026-09-26 (macOS 26.6 current, macOS 27 in beta).

- **Where it runs:** Mac only, macOS 26 or later with Apple Intelligence on. Everywhere else the action is hidden, and Settings → AI says why.
- **What you get:** a card at the top of the thread with a one- or two-sentence gist, the key points, and anything asked of you (with its deadline). Every point and request links to the message it came from.
- **Speed:** a summary you already made is a SQLite read and shows as soon as the thread opens. A new one streams in as the model writes.

## Using it

| | |
|---|---|
| Summarize | **Summarize** in the thread toolbar (icon-only in the preview pane), **⇧S**, or ⌘K → "Summarize conversation". Pressing ⇧S again hides the card, and a third press shows it again. |
| While it runs | "Reading part 2 of 4" for long threads, then the summary streams in. **Stop** cancels it. |
| Sources | Each point has a `#3 · Maya` chip. Clicking it opens that message, moves the cursor to it and flashes it. |
| Regenerate | Summarizes again. The temperature is 0.3, not greedy, so the wording can change. |
| Cached | A stored summary shows when you open the thread. If new mail arrived since, it's dimmed with "New messages since this summary · Update". |
| Settings → AI → Thread summaries | An on/off switch (on by default: the feature only runs when you ask, and only on this Mac). When the Mac can't run it, this row explains why, and the switch is disabled. |

## Architecture

```
UI  features/summary/ (SummaryCard, SummarizeButton, SummarySettings, state.ts, model.ts)
 │   api.summaryAvailability / cachedSummary / summarizeThread / cancelSummary / prewarmSummarizer
 │   ◀── penguin://summary-progress {stage: reading|writing, step, steps, partial}
 ▼
src-tauri/src/summary/
   mod.rs       commands, one summary at a time, cancel switches, the settings gate
   pipeline.rs  plan → map (notes per chunk) → reduce (streamed) → retry smaller on overflow;
                guided first, then plain text with permissive guardrails
   engine.rs    Engine trait: availability / prewarm / start(request) → events + cancel
   apple.rs     macOS: the C ABI of the Swift bridge (callback + context pointer)
 ▼
swift/PenguinAI  (SwiftPM static library, built by build.rs through swift-rs, macOS only)
   SystemLanguageModel availability, contextSize, tokenCount(for:), LanguageModelSession,
   @Generable ThreadDigest, streamResponse snapshots, prewarm, error mapping
penguin-core/src/summary.rs   input building, token estimate, chunking, prompts, parsing,
                              citations → message ids, version_key (pure, tested on Linux)
penguin-core/src/store_summaries.rs   the `summaries` cache table
```

Rust owns every decision about *what* the model sees (instructions, prompts, chunking, parsing, citations), so it's unit-tested on the Linux build box. The Swift side runs one generation and reports back.

### The bridge (Swift ⇄ Rust)

`build.rs` uses [swift-rs](https://github.com/Brendonovich/swift-rs)'s `SwiftLinker` to `swift build` the `PenguinAI` package for macOS and link `libPenguinAI.a`, along with the Swift runtime search paths. The functions are `@_cdecl` and `public`. They're `public` because Xcode 27's SwiftPM release builds give non-public `@_cdecl` symbols local visibility ([swift-rs#81](https://github.com/Brendonovich/swift-rs/issues/81)). swift-rs 1.0.8 also re-globalizes them with rustup's `llvm-tools`, which CI installs.

An `@_cdecl` function can't be `async` ([swift-rs#31](https://github.com/Brendonovich/swift-rs/issues/31), where the recommended pattern is a `Task` plus a C completion callback). So `penguin_ai_generate(request_json, ctx, callback)` returns a handle at once and starts a `Task`. The Task calls `callback(ctx, kind, json)` once per snapshot, then exactly once with `done` or `error`. Rust passes a leaked `Box<UnboundedSender>` as `ctx` and frees it on that terminal event (`apple.rs`), so the pointer is never used after it's freed, even if the summary was cancelled and the receiver dropped. Cancelling cancels the Task. Rust also stops waiting at once, so Stop works even if the framework takes a moment to notice.

The only data that crosses the bridge is plain bytes (UTF-8 JSON). No swift-rs runtime types are used.

### Older macOS and older SDKs

- **The app's minimum macOS is unchanged.** `tauri.conf.json` doesn't set one, so it's Tauri's default. The Swift package is compiled for the Rust build's own deployment target (11.0 on Apple silicon), not 26.
- Every use of the framework is behind `if #available(macOS 26.0, *)`, and `build.rs` links FoundationModels **weak** (`-weak_framework`, [Apple: weak linking](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPFrameworks/Concepts/WeakLinking.html)). The framework only exists from macOS 26 ([Apple: "The availability of the FoundationModels framework starts at 26.0"](https://developer.apple.com/documentation/foundationmodels/updating-prompts-for-new-model-versions)), so on older macOS the app still launches and the bridge reports `osTooOld`. CI checks for `LC_LOAD_WEAK_DYLIB` on the built binary.
- **The Swift concurrency runtime.** With a deployment target below macOS 12, the compiler references it as `@rpath/libswift_Concurrency.dylib`.
  - Xcode puts `/usr/lib/swift` on an app's run path for this, and a rustc link doesn't. CI's first run crashed on exactly that: *"Library not loaded: @rpath/libswift_Concurrency.dylib … no LC_RPATH's found"*.
  - So `build.rs` adds the `/usr/lib/swift` run path and links the library weak, as Xcode 26 does, since macOS 11 has no copy.
  - The bridge touches concurrency types only behind `#available(macOS 26.0, *)`.
  - CI checks the run path and both weak loads, then runs the release smoke binaries on a macOS 15 runner. There they must answer `osTooOld`, not crash.
- `#if canImport(FoundationModels)`: a build with an SDK older than Xcode 26 still compiles, and reports `notBuilt`.
- `#if compiler(>=6.3)` (Xcode 26.4+, macOS 26.4 SDK): `contextSize`, which is back-deployed to 26.0 ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/contextsize)), and `tokenCount(for:)`, which needs macOS 26.4 at runtime ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/tokencount(for:))). Without them, the context is taken as 4,096 and token counts are estimated.
- `#if compiler(>=6.4)` (Xcode 27): the new error types. `LanguageModelSession.GenerationError` is deprecated in 27. [Apple](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/generationerror): *"Apps built with Xcode 26 will continue to catch this error until you rebuild with Xcode 27."* A build made with Xcode 27 catches `LanguageModelError`, `LanguageModelSession.Error` and `SystemLanguageModel.Error` on macOS 27, and still catches `GenerationError` from macOS 26.
- CI proves each branch of these guards: the Swift package alone on Xcode 26.0.1 and 26.3, and the whole app on the default Xcode 26 and on Xcode 27. The smoke test pins which APIs each one compiled in.

## Decisions, and the Apple sources behind them

### Availability, and why the action hides

`SystemLanguageModel.default.availability` is `.available` or `.unavailable(reason)`, with reasons `deviceNotEligible`, `appleIntelligenceNotEnabled` and `modelNotReady` ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/availability-swift.enum/unavailablereason)). Apple's samples keep a fallback case for reasons added later, and so does Penguin (`unknown`).

Penguin adds three reasons of its own: `osTooOld` (macOS before 26), `notBuilt` (compiled without the SDK) and `unsupportedPlatform` (Linux builds).

What each reason means for the user:
- Apple Intelligence needs a Mac with M1 or later.
- Device language and Siri language set to the same supported language.
- Up to 14 GB free (M3 and later with 12 GB or more of memory), otherwise up to 8 GB.
- Not available on devices bought in mainland China ([Apple Support 121115](https://support.apple.com/en-us/121115)).
- Apple Intelligence follows the **Siri** language, not the system language ([Apple engineer, developer forums](https://developer.apple.com/forums/thread/805378)).

The toolbar hides Summarize whenever it can't run, and Settings → AI → Thread summaries gives the reason once, for example: "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri".

Penguin checks availability at launch, again each time Settings → AI opens, and at most once a minute while threads open. That catches Apple Intelligence being switched on, or finishing its download, while Penguin runs.

### The context window, and map-reduce

- **The limit.** *"Apple's on-device foundation model has a context window of 4096 tokens per LanguageModelSession"* ([TN3193](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)). `contextSize` is *"the total number of tokens that can be used in a single session, including both input prompts and generated responses"* ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/contextsize)). Penguin reads it when the SDK allows, since Apple suggests adapting to the hardware in 27 ([WWDC26 session 241](https://developer.apple.com/videos/play/wwdc2026/241/)). Otherwise it uses 4,096.
- **The budget.** Instructions, prompt wording, the generation schema (350 tokens set aside) and the answer (`maximumResponseTokens` 700) come off the top. What's left is for messages.
- **The estimate.** TN3193 gives about 3 to 4 characters per token for Latin scripts and about 1 for Chinese, Japanese and Korean. Penguin counts 3 ASCII characters per token and 1 per other character, which deliberately overestimates.
- **The exact count.** On macOS 26.4+, the bridge measures instructions, prompt and schema with `tokenCount(for:)` before generating. If the request won't fit, it answers `contextExceeded` with the real numbers, and Rust re-plans to that ratio (with 10% to spare). Without 26.4, the model's own `exceededContextWindowSize` triggers a re-plan at 60% of the budget. After three plans, Penguin gives up with "too long".
- **Map-reduce (TN3193's recommendation).** *"separating the article into smaller chunks … summarizing each chunk with a new session, combining the results together, and then repeating this process"*. Each chunk gets a fresh session (the "map" step), and the notes are then summarized (the "reduce" step). If the notes don't fit one call, they're grouped and reduced again.
  - The notes keep the `#n` message numbers, so citations survive the reduce step.
  - A single huge message is split into numbered parts.
  - A thread that would take more than 6 map calls keeps the first message and the newest ones that fit. The prompt says how many were skipped, and so does the card's footnote.
- **New sessions each time.** The model forgets everything between sessions ([WWDC25 session 301](https://developer.apple.com/videos/play/wwdc2025/301/): catch the overflow and start a new session), so every call is self-contained.

### What the model reads

The model reads each message's **authored** text, oldest first:
- Quoted history is cut by the same `split_quoted` used for search ranking.
- Drafts are left out, and so are spam and trash unless that's all the thread has.
- A reply that only quotes adds nothing and is left out.
- The signature after `-- `, "Sent from my iPhone" and link targets are dropped. Links are kept as `[link: host]`.

Each message gets a short header: `[#3 · Fri 2026-09-12 14:03 · Maya Lin <maya@northwind.example>]`, with "You" for your own addresses.
- The weekday lets the model resolve "by Friday" against the message date.
- The instructions give today's date.
- A forward is marked "(forwarded; the text below was written by someone else)", so the forwarder isn't credited with it.
- A message stored headers-only (older than the sync window) contributes its snippet, marked "preview only".

### Instructions versus prompt (prompt injection)

The rules live in the **instructions** and the mail goes only in the **prompt**.
- The model gives instructions precedence.
- Apple warns: *"don't include input from people or any unverified input in the instructions … vulnerable to prompt injection"* ([Improving the safety of generative model output](https://developer.apple.com/documentation/foundationmodels/improving-the-safety-of-generative-model-output)).
- The instructions start with the model's role, which Apple says reduces over-blocking: *"The very beginning of an Instructions string is an effective place to give the model a clear role"*.
- They tell the model to use only what the messages say, cite a message number for everything, and ignore any instructions inside the mail.
- Apple also advises keeping prompts and instructions to 1 to 3 paragraphs (TN3193), and ours are short.

### Output: `@Generable`, property order, citations

`ThreadDigest { points: [DigestPoint{source, text}], asks: [DigestAsk{source, text, due}], gist }`, generated with guided generation ([Generable](https://developer.apple.com/documentation/foundationmodels/generable), [`@Guide` constraints](https://developer.apple.com/documentation/foundationmodels/generationguide)).

- **Property order is deliberate.** *"properties are generated in the order they are declared on your Swift struct. This matters both for animations and for the quality of the model's output"* ([WWDC25 session 286](https://developer.apple.com/videos/play/wwdc2025/286/)), and a property can be *"influenced by another property"* ([session 301](https://developer.apple.com/videos/play/wwdc2025/301/)). So the points and requests come first and the gist last: it summarizes what the model has just written down. In each item, the source number comes before the text, so the model commits to a message and then describes it.
- **Caps.** `.maximumCount(6)` points and `.maximumCount(4)` requests. `due` is a plain string (empty when none), not an optional.
- **Why numbers, not message ids.** The model cites `#n`, which it can copy reliably. A 16-hex-digit Gmail id is easy to garble. Rust maps the numbers back to message ids (`resolve`).
  - A number that doesn't exist keeps its text but links nowhere.
  - Duplicates and blanks are dropped, and lengths are capped.
  - Guided generation guarantees the *shape*, not that the citation is *right*. That's why every item links to its source, and why the footnote says to check the linked messages.

### Streaming

`streamResponse(to:generating:)` yields **snapshots, not deltas**: each one is the whole `PartiallyGenerated` digest so far ([session 286](https://developer.apple.com/videos/play/wwdc2025/286/): *"Instead of raw deltas, we stream snapshots"*; element type [`ResponseStream.Snapshot`](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/responsestream), `.content`).
- The bridge sends each snapshot as JSON, and Rust resolves it and emits `penguin://summary-progress`.
- The card renders the partial summary: points appear first, then the gist.
- Map steps don't stream: they use `respond`, since the user only sees "Reading part n of m".

### Guardrails and email

Email is exactly the content that trips safety filters: medical results, legal disputes, newsletters about crime. Guardrails check both input and output ([WWDC25 session 248](https://developer.apple.com/videos/play/wwdc2025/248/)). Apple has cut false positives over time (26.4: *"Reduce the possibility of blocking benign content with improved guardrails"*, [updates](https://developer.apple.com/documentation/updates/foundationmodels); more in 27 per session 241), but they still happen.

`SystemLanguageModel.Guardrails.permissiveContentTransformations` exists for this case: *"lets the model handle potentially unsafe content, such as summarizing a news article. In this mode, requests you make to the model that generate a `String` will not throw guardrailViolation(_:) errors … When you generate responses other than `String`, this mode behaves the same way as default mode"* ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/guardrails/permissivecontenttransformations)). Since it only relaxes **String** output, Penguin:

1. generates guided (`ThreadDigest`) with the default guardrails;
2. on `guardrailViolation` or `refusal`, asks again for **plain text** with `.permissiveContentTransformations`, in a fixed line format:
   ```
   POINT #n: …
   ASK #n (due …): …
   GIST: …
   ```
   Rust parses this format, even partially while it streams (`parse_text_draft`);
3. if what comes back isn't the format (Apple: in this mode *"the model may still sometimes refuse … in which case it generates a String refusal message"*, and *"You might not be able to programmatically determine whether a string response is a normal response or a refusal"*), it's treated as a refusal. The card then says "Apple's on-device model declined to summarize this conversation", with Try again.

This stays within Apple's [acceptable use requirements](https://developer.apple.com/apple-intelligence/acceptable-use-requirements-for-the-foundation-models-framework/). Summarizing the user's own mail is allowed, and the permissive mode is Apple's own documented option for transforming content, not a way around a guardrail.

### Use case and adapter

The model is `SystemLanguageModel.default` (use case `.general`). The only other built-in use case, `.contentTagging`, *"always responds with tags"* ([docs](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/usecase)). That's for classification, not prose, so it's the wrong adapter for summaries. It could suit a later labeling feature.

### Prewarm

`prewarm(promptPrefix:)`: *"You should only use prewarm when you have a window of at least 1 second before the call to a respond method"* ([docs](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/prewarm(promptprefix:))). The code-along session recommends prewarming *"just after the user gives a strong hint"* ([WWDC25 session 259](https://developer.apple.com/videos/play/wwdc2025/259/)).

Penguin prewarms when the pointer or focus lands on Summarize, at most once a minute. The bridge keeps that session (with the same instructions) for the next guided request, for up to five minutes. It doesn't prewarm on every thread open, which would load the model for threads nobody summarizes.

### Errors → what the user sees

| Framework (26 / 27 SDK) | Penguin |
|---|---|
| `exceededContextWindowSize` / `LanguageModelError.contextSizeExceeded` | Re-plan smaller (automatic), then "too long to summarize on this Mac" |
| `guardrailViolation`, `refusal` | Plain-text permissive retry, then "declined to summarize" |
| `unsupportedLanguageOrLocale` | "doesn't support this conversation's language yet" |
| `assetsUnavailable` / `SystemLanguageModel.Error` | "still getting ready on this Mac" |
| `rateLimited`, `concurrentRequests`, `timeout` (27) | "busy, try again in a moment" |
| Task cancelled | Quietly returns to what was shown before |

Penguin never forwards the framework's error text, because its descriptions can quote generated text. Only the error type name goes through, for "other".

### Concurrency and background

- Only one summary runs at a time (`Summarizer.gate`). A session serves one request at a time (`isResponding`, `concurrentRequests`), and the Neural Engine is shared anyway.
- Asking again for the same thread cancels the run in progress.
- Streaming happens only while the user is looking. Apple advises against streaming in the background (*"use the non-streaming respond … to reduce the likelihood of encountering rateLimited"*, [streamResponse](https://developer.apple.com/documentation/foundationmodels/languagemodelsession/streamresponse(to:options:))), and Penguin doesn't summarize in the background yet.

### Cache

`summaries(account_id, thread_id, version, created_at, data)`, one row per thread, appended at the end of `MIGRATIONS`.

- **The version key.** `version` is `summary::version_key`: FNV-1a over `SUMMARY_FORMAT` and each included message's id, preview-only flag and cleaned text.
  - These change it: a reply, a headers-only body arriving, or a prompt/format change (bump `SUMMARY_FORMAT`).
  - These don't: labels, read state, or draft edits.
- **Reading it.** `cached_summary` recomputes the version from the stored thread (local, O(messages)) and returns the row with `stale` set if the version differs.
- **Cleanup.** Rows go when the account is removed or the thread's last message is deleted.
- **Model updates.** The model changes with OS updates ([Apple: updating prompts for new model versions](https://developer.apple.com/documentation/foundationmodels/updating-prompts-for-new-model-versions); macOS 27 ships AFM 3, [Apple ML research](https://machinelearning.apple.com/research/introducing-third-generation-of-apple-foundation-models)), but an existing summary stays valid, since it describes the same mail. Regenerate makes a new one.

### Privacy and logging

- Everything is on the device. Apple describes the model as ~3B parameters, 2-bit quantization-aware-trained ([Apple ML research, 2025](https://machinelearning.apple.com/research/apple-foundation-models-2025-updates)).
- The logs record account and thread ids, message counts, timings and an outcome code. Never a subject, a body, a prompt, or anything the model wrote.
- Summaries are stored only in the local database, next to the mail they describe.

## The model

- **What it is.** About 3B parameters, 2-bit weights with 4-bit embeddings and an 8-bit KV cache. It's tuned for *"summarization, extraction, classification … not designed for world knowledge or advanced reasoning"* ([WWDC25 session 286](https://developer.apple.com/videos/play/wwdc2025/286/), [Apple ML research 2025](https://machinelearning.apple.com/research/apple-foundation-models-2025-updates)).
- **Training versus the window.** Its training used sequences up to 65K tokens, but the framework's window is 4,096 (TN3193), and that's what the design assumes.
- **macOS 27.** It brings AFM 3 Core, and on the most capable Macs AFM 3 Core Advanced ("20-billion-parameter … activating just 1 to 4 billion", [Apple ML research 2026](https://machinelearning.apple.com/research/introducing-third-generation-of-apple-foundation-models)). Nothing here depends on which variant runs.

## Testing

- **penguin-core** (`summary_tests.rs`, Linux): input building (authored text, headers, time zones, drafts, spam and trash, forwards, headers-only messages), cleaning, the token estimate, the budget, chunking (fits, splits, omissions), prompts, the notes and reduce round trip, the text-format parser, citation resolution and caps, `version_key` (content yes, labels no), and the cache table (round trip, stale, thread and account deletion).
- **src-tauri** (`summary/tests.rs`, Linux and macOS CI): the pipeline against a scripted engine. It covers streaming, map-reduce order and progress, re-planning after a measured overflow, giving up, the guardrail fallback to permissive text, refusals in both modes, empty answers, the error mapping, cancellation (the engine is told to stop), and the request JSON the bridge decodes.
- **UI** (`tests/summary.test.ts`): card states (cached, stale, starting, reading, writing, done, error, cancel, close and reopen), the ⇧S toggle, source labels, the footnote, and the unavailable texts.
- **macOS CI** (`.github/workflows/macos-check.yml`): the builds described above, a weak-link check, no `@rpath` Swift runtime, and `examples/summary_bridge`. Runners are VMs, where *"Apple Intelligence doesn't support running on VM"* ([Apple DTS](https://developer.apple.com/forums/thread/787445)), so CI proves the bridge compiles, links and answers `unavailable` cleanly. It doesn't exercise the model.
- **On a Mac:** `cargo run -p penguin-desktop --example summary_bridge` prints the build features and availability, then streams one real guided summary of a made-up message.
- **Mock UI:** `npm run dev:mock` streams a fake summary built from the thread's own messages. `?ai=notEnabled|noDevice|notReady|old|refuse|slow` shows the other states.

## Not done yet

- Background summaries for new long threads. They'd need non-streaming `respond` and a rate-limit policy.
- Incremental re-summarizing (cached partial + new messages) instead of re-reading a long thread after a reply.
- A language check up front (`supportsLocale`) before the model reports `unsupportedLanguageOrLocale`.
