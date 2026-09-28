# Welcome to Penguin (first-run setup)

Adding an account is handled by `features/onboarding`. After that comes a short **Welcome** setup, which appears once, right after the first account is connected and syncing has started. Mail keeps downloading behind it. The setup asks a handful of questions, and every answer is preselected with the default.

- Code: `apps/desktop/src/features/welcome/` (`model.ts` holds the rules, `Welcome.tsx` the screens, `index.tsx` the host that App mounts, `state.ts` open and close).
- Setting: `Settings.welcomeCompleted` (settings.rs ⇄ types.ts).
- Command: `start_model_download`.
- Tests: `tests/welcome.test.ts`; `settings::tests::welcome_shows_on_new_installs_only`; `semantic::tests::a_new_install_waits_for_the_welcome_before_downloading`.

## The flow

It has 5 short screens, or 6 on a Mac with Apple Intelligence. A step list sits at the top, and a footer holds **Use defaults** (Esc), **Back** (⇧↵) and **Next** (↵).

| # | Screen | Asks | Default |
|---|---|---|---|
| 1 | Search by meaning | On or off, with the plain explanation: ~217 MB one-time download of Google's EmbeddingGemma, background indexing that pauses on battery saver and when the Mac is hot, and the model and licence line (Gemma Terms of Use) | On |
| 2 | Apple Intelligence (only when `summary_availability` says available) | Thread summaries, Write with AI, Suggested replies, Ask reads your question: four switches | As in Settings: on, on, off, on |
| 3 | How much mail to keep on this Mac | The sync window choices (1, 3, 6 months, 1 or 2 years, Everything), with the trade-off: disk space and first-download time against full-text search of older mail. It notes that shrinking never deletes anything. | 6 months |
| 4 | Your inbox | List style (5 cards with small CSS previews), Split Inbox on or off with two tab sets (Important · Calendar · News, or People · Notifications · News), Start in Floe mode | Quiet, off, off |
| 5 | Look | Theme (System, Light, Dark), sidebar theme swatches, sidebar text size. `TODO(dark shade)` marks where the dark-shade choice goes once `darkShade` lands. | System, Graphite, Default |
| 6 | Keys and notifications | A "try it" list that J and K move, Show key hints, Shortcut coach, Notify me about new mail | off, on, off |

- **Choices apply immediately.** Each one is saved as it's picked, so the app behind the sheet already shows it. Esc and **Use defaults** end the setup with whatever has been chosen so far. Everything not reached keeps its default.
- **Keyboard.** Each screen focuses its heading, so ↵ means Next and ⇧↵ means Back. Tab moves between controls, and ← → move inside a group of options. Space toggles a switch. J and K work on the last screen. While the sheet is open, no key reaches the app's single-key shortcuts.
- **Notifications.** macOS asks for permission only when the switch is turned on, never merely because the screen was shown.
- **Reopen.** Use ⌘K → "Set up Penguin…" or Settings → General → "Set up Penguin…". Reopening changes nothing until something is picked.

## Nothing big downloads before the question

Before this change, a new install started the ~217 MB model download at launch, before anyone had been asked. Now the model download is held back by a gate:

- `Semantic::downloads_allowed` (src-tauri/src/semantic/mod.rs) starts out as `welcomeCompleted`. While it is false and the model files aren't on disk, the indexer reports `paused` with the reason "you finish setting up Penguin" and waits. Settings → Search shows "Paused while you finish setting up Penguin".
- **Leaving screen 1 with search by meaning on** calls `start_model_download`, which opens the gate. The download then runs while the remaining screens are answered, and its progress shows in the footer.
- **Finishing or skipping** sets `welcomeCompleted`, and `update_settings` opens the gate. Skipping keeps the defaults, including search by meaning on, so a skip downloads the model too.
- **Turned off on screen 1:** nothing downloads, now or later, until it is switched on in Settings.
- **Files already on disk** (an existing user's model) are used whatever the gate says.

## Who sees it

`welcomeCompleted` defaults to false, and `SettingsState::settle_welcome` runs at startup, before the indexer is built:

- **The settings file has no `welcomeCompleted` key** (the file is missing, or was written by an older build):
  - With at least one account, it is an existing install, so the setting is set to **true**. Every current user skips the welcome, and their model isn't held back.
  - With no accounts, it is a new install. `false` is written to the file straight away, so an account added followed by a quit before the setup ends still shows the setup on the next launch.
- **The key is present:** it is left alone.
- **In the UI,** `WelcomeHost` opens the setup when `shouldAutoOpen` holds: the flag is false, at least one account exists, and the layout isn't the phone.

## Why screens and not one page

The brief allowed 4–6 short screens or one scrolling page. The research below leans toward fewer, more compact steps, and one page would have been defensible. Screens were chosen for four reasons:

1. **The download question has to come first and be answered on its own.** Apple: "Don't let large downloads hinder onboarding." Giving it a screen of its own lets the answer start the download at once, and the rest of the setup then runs while it fetches. On a single page, the choice would only be committed at the end.
2. **Previews need room.** The list-style, theme and sidebar previews don't fit on one page without making it long.
3. **↵ to continue, as Superhuman teaches.** One Enter per topic is the keyboard lesson itself, and the J/K moment belongs to it.
4. **NN/g's wizard advice fits:** use wizards "for novice users or infrequent processes (e.g., configuration or setup)", and show "a list or a diagram of the steps involved and highlighting the current step". The same article warns that wizards annoy experts. That is why there are only 5–6 screens, why each screen is one topic, and why **Use defaults** sits on every screen and ends the setup there.

## Sources and what was taken from each

Each source is quoted from its own page (fetched 2026-09-28) unless it is marked otherwise.

- **Apple Human Interface Guidelines, [Onboarding](https://developer.apple.com/design/human-interface-guidelines/onboarding):**
  - "design a flow that's fast, fun, and optional". Hence **Use defaults** and Esc on every screen.
  - "Provide reasonable default settings so most people can immediately start interacting… without performing additional configuration". Hence every answer is preselected.
  - "Don't let large downloads hinder onboarding". Hence the model downloads in the background from screen 1 on, and the inbox never waits for it.
  - A skipped flow shouldn't come back on later launches, but should stay easy to find in settings. Hence the `welcomeCompleted` flag, ⌘K "Set up Penguin…" and the button in Settings → General.
- **Apple HIG, [Privacy](https://developer.apple.com/design/human-interface-guidelines/privacy):** "Ideally, wait to request permission until people actually use an app feature that requires access". Hence the notification permission prompt appears only when the switch is turned on.
- **NN/g, [The Power of Defaults](https://www.nngroup.com/articles/the-power-of-defaults/):** "Users rarely utilize fancy customization features, making it important to optimize the default user experience". Hence the defaults are the product: skipping is a first-class path, and it gives the recommended setup, including search by meaning.
- **NN/g, [Mobile-App Onboarding: An Analysis of Components and Techniques](https://www.nngroup.com/articles/mobile-app-onboarding/):**
  - Content customization suits initial onboarding, but "visual-design customization, such as selecting a color scheme, doesn't belong in onboarding". The Look screen goes against this because Justin asked for it. It is kept to one short screen near the end, its default follows the system, and Esc skips it. If it proves to be friction, it is the first screen to drop.
  - A good example shows Skip "along with a progress indicator". Hence the step list.
- **NN/g, [Onboarding Tutorials vs. Contextual Help](https://www.nngroup.com/articles/onboarding-tutorials/):** launch tutorials are "push revelations" that are "hard to remember when the user needs it". Hence there is no feature tour: each screen is a decision, not a lesson. Keys are taught in context by the shortcut coach, which the setup only switches on or off.
- **NN/g, [Wizards: Definition and Design Recommendations](https://www.nngroup.com/articles/wizards/):** quoted above. It supplied the step list plus Next and Back.
- **Superhuman, [Superhuman's Onboarding Playbook](https://review.firstround.com/superhuman-onboarding-playbook/)** (First Round Review, by Gaurav Vohra, Superhuman's former growth lead):
  - "We required users to hit 'enter' to get started. Clicking buttons made them jiggle helplessly, encouraging users to abandon the mouse." Hence ↵ as the primary action, with its key cap always shown.
  - The "piano lesson" idea of turning movement into muscle memory. Hence the small J/K "try it".
  - Superhuman personalizes by role. That was not copied: the Split Inbox tab sets are the nearest equivalent.
- **Linear, [Keyboard shortcuts help](https://linear.app/changelog/2021-03-25-keyboard-shortcuts-help)** (changelog, 2021-03-25): shortcuts are taught through `?`, a panel that can be opened at any time. Hence the Keys screen points at ⌘K and ? rather than listing keys. Linear hasn't published its own account of its onboarding flow. A secondary write-up says it opens with a theme choice and ⌘K practice; that isn't used as evidence here.
- **Raycast, [Be obsessed with feedback, not metrics](https://www.raycast.com/blog/feedback):** "We don't require a login to use Raycast", and during onboarding it hints one useful command. The [Quickstart](https://manual.raycast.com/quickstart) teaches by doing ("Press your Raycast hotkey, start typing…"). Hence no account and a do-it-now key moment.
- **Arc (The Browser Company):** no first-party write-up about onboarding was found, and its Help Center returned 403. A secondary interview ([Inverse](https://www.inverse.com/input/design/the-browser-company-arc-design-interview)) describes moving from manual onboarding to self-setup, and a first run meant to feel like "opening a gift". It is noted for tone only, not relied on.

## Mock and screenshots

- `npm run dev:mock`, then `?welcome=1`. This opens the setup as on a new install: `welcomeCompleted` is false, and `semantic_status` is paused "while you finish setting up Penguin". After screen 1 it shows a 30-second download, and finishing sets the flag and closes the setup.
- ⌘K "Set up Penguin…" opens it in any mock session.
