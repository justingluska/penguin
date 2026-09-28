# Penguin — design proof of concept (v0.1)

Five static, high-fidelity mockups of Penguin: an open-source desktop email client that puts speed and the keyboard first. It targets Google Workspace, with several accounts in one inbox. The pages are plain HTML and CSS. The only JavaScript is an icon sprite and a theme switch.

Open `index.html` to browse the screens. Add `?theme=light` or `?theme=dark` to any page URL to pick a theme, or press **T** on any page to switch.

```
design/
  index.html          gallery with thumbnails (follows ?theme=)
  01-inbox.html       unified inbox, three panes
  02-thread.html      reading a thread + context panel
  03-search.html      search overlay (the main screen)
  04-compose.html     composer over the inbox
  05-command.html     ⌘K command palette over the empty-inbox screen
  penguin.css         tokens (dark + light) and components
  penguin.js          icon sprite (Lucide-style, 1.75 stroke) + T to switch theme
  screenshots/        each screen in dark and light, 1440×900 at 2x
```

## Design principles

1. **Speed is part of the design.** Every action in the core triage loop takes one key: `E` done, `H` snooze, `R` reply, `L` label, `#` delete, `J`/`K` move, `/` search, `C` compose. Two-key `G`-sequences navigate. Anything else is two keystrokes away through ⌘K.
2. **Every action shows its shortcut.** Buttons, hover actions, menu items and palette rows all display a monospace key hint. The status bar at the bottom of each screen repeats the keys for that context, so you learn them just by using the app.
3. **Search is the main screen.** People search their mail to find something they have already seen, so the search screen is built for that. It shows the best match first, with the matching sentence (not the opening line of the email). Operators are highlighted as you type, and each one appears as a removable chip. Text inside attachments is searchable. The screen always states how long the search took and what was searched ("38 ms · 142,318 messages indexed locally"). It never quietly falls back to the server; searching the server is a separate, clearly labeled option.
4. **One accent color, used sparingly.** Blue marks only the text caret, the unread bar, focus, and active counts. Everything else is neutral gray at different transparencies. The primary button is black in light mode and white in dark mode, rather than blue.
5. **Color means identity, not decoration.** Each account has its own color (Northwind violet, Harbor Labs green, Personal orange), shown as a small dot on every row. Labels are neutral outlined chips with a small color swatch, so a long list stays calm. AI is always shown in violet and always labeled.
6. **Dense but readable.** List rows are 40px tall. Every control is 32px. Text is 13–14px. There are no photos of senders; initials appear only where they help (the reading pane, the context panel, recipients).
7. **Honest AI.** The AI answer in search can be turned off (`⌥A`), says how many emails it used, cites each fact with a numbered source, and runs on the device. "Draft with AI" in the composer is a quiet ghost button, never the main action.
8. **Two different empty states.** Clearing your inbox gets a small celebration, with a penguin on an ice floe. This is kept separate from the first-run onboarding screen.

## Tokens

The system has three layers. Components only ever use the **semantic** layer.

**Scales.** Gray steps 1–12 and transparent gray steps a1–a12, plus tone scales (green, blue, amber, red, violet, orange, cyan) at steps a3, a4, a5 and a11. Dark mode is the default and is declared on `:root, [data-theme="dark"]`. `[data-theme="light"]` overrides it. The dark transparent grays have a slight blue tint, and in dark mode gray-a2 is a *dark* wash (surfaces get darker in dark mode, never lighter).

**Semantic layer (selection).**

| Token | Dark | Light | Used for |
|---|---|---|---|
| `--background` | `#000` | `#fdfdfd` | app canvas |
| `--bg-panel` | `#0c0d0f` | `#fff` | palette, composer, search overlay |
| `--bg-interactive` | gray-a2 | gray-a2 | default fill for every control |
| `--bg-interactive-hover` | gray-a3 | gray-a3 | hover state for every control |
| `--bg-row-hover` / `--bg-row-selected` | a2 / a3 | a1 / a2 | message rows (tuned per theme) |
| `--bg-accent` | `#fff` | `#000` | primary button |
| `--text-emphasis` / `default` / `muted` / `faint` | gray-12 / 11 / 10 / 9 | gray-12 / 11 / 7 / 6 | four text levels |
| `--border-default` / `subtle` / `interactive` | gray-3 / gray-2 / gray-a3 | same | card outline / hairline / control edge |
| `--accent` | blue-a9 `#0090ff` | same | caret, unread bar, focus |
| `--bg-highlight` | amber-a4 | amber-a5 | matched search terms |

**Tone rule.** Every tinted element has a fill of `tone-a3` and text of `tone-a11`. One class (`.t-green`, `.t-violet`, …) sets `--t3/--t4/--t5/--t11`, and badges, avatars, file icons, callouts and the AI card all read from those variables.

**Type.** Inter, weights 400, 500 and 600 only, with `cv11` and `ss01` turned on. Geist Mono for key hints, search operators and snippet names. Sizes: 11 / 12 / 13 / 14 / 16 / 18 / 20 / 22px. Letter-spacing tightens as size grows: −0.2px at 16–18px, −0.45px at 22px, −1px at 32px.

**Radii.** 6px for small badges and key hints. 8px for small buttons, chips and file-list rows. 12px for every 32px control, message row and menu item. 16px for panels, the reply box and callouts. 24px for cards.

**Heights.** Controls are 32px, small controls 24px, message rows 40px, pane headers 56px, the status bar 34px.

**Shadows.** Only floating surfaces have a shadow (`--shadow-panel`, `--shadow-lg`). Cards, rows and inputs are flat and separated by hairline borders.

**Motion.** Controls fade over 200ms using `cubic-bezier(.4,0,.2,1)`. The command palette and search overlay open and close with **no animation**, because they are used dozens of times a day. Hover fills change instantly.

## What each screen shows

**01 — Unified inbox.** Three panes: sidebar, list and reading preview. The sidebar has an account switcher (the logo plus the three account dots), a primary compose button (`C`), a search button (`/`), the main views (Inbox, Screener, Starred, Snoozed, Sent, Drafts, Done), labels (square color swatches) and accounts (round dots, with `+` to add one). The inbox is split into tabs: Important, Other, Newsletters and Calendar. A dashed strip at the top of the list says 4 new senders are waiting in the Screener (`G` then `N`). Rows are grouped by day. Unread rows have semibold text and a 2px blue bar on the left. The selected row is filled. The hovered row shows done, snooze, reply, label and delete, each with its key. The preview pane has done, snooze and label buttons with keys, plus a quick-reply field.

**02 — Thread.** Older messages are collapsed into rows. A dashed "1 more message" divider stands in for hidden messages in between. The latest message is open, with its attachments as file cards. The reply box stays pinned at the bottom. It shows which account it will send from, has Send (`⌘↵`), Send later, and Snippets (`;`), and a quiet "Draft with AI" (`⌘J`). The context panel on the right shows the sender's role and company, their local time relative to yours, how long you have been emailing, a shortcut to "All mail with Priya", recent threads, shared files, who is in the thread, and an upcoming meeting.

**03 — Search (main screen).** A search overlay opened with `/` or ⌘K. The query `from:mike has:pdf lease date:"last spring"` is highlighted as you type: operators in blue monospace, and the `date:` value underlined to show it was understood. Dates only come from `date:` (or `before:`/`after:`); a bare word like "february" is searched as text, and a hint under the input offers to turn it into `date:february`. The row below shows how the query was read, as removable chips (From Mike Delgado, Has PDF, Date Mar 1 – May 31 2026, and "lease" with related words), next to a green badge reading `38 ms · 142,318 messages indexed locally`. Filters on the left cover account, date, attachment type, label and saved searches, with counts. The results are grouped:
- **Best match:** the matching sentence, "3 matches in thread", and which PDF page matched.
- **Attachments:** includes matches on text inside the PDFs.
- **Threads:** matched words highlighted, with a note on how many older matches fall outside the date range.
- **People:** shows that "mike" matched Mike Delgado rather than Mike Osei.
- **AI answer:** violet, clearly labeled, can be turned off with `⌥A`, runs on the device, and cites sources [1] and [2].
- **Recent searches.**

The footer lists the keys (`↑↓`, `↵` opens at the match, `⌘↵` opens in a split view, `Tab` jumps to the next group, `⌘G` goes to the next match), states the scope ("All 3 accounts · 100% local"), and offers a separate "Also search server" option.

**04 — Compose.** A modal over the dimmed inbox. There is a sender picker for the three accounts (`⌥1–3`), recipient chips with initials, and a subject line. The date phrase in the body is underlined to show it was recognized. Typing `;rev` opens the snippet picker, with a live preview, an `{variable}` placeholder, and `Tab` to move between variables. The toolbar has Send (`⌘↵`) with a dropdown, a suggested send-later time (`⌘⇧↵`), formatting, snippets, and a quiet "Draft with AI". A footer notes the 10-second undo window and points out that 8:00 AM here is 1:00 PM for Priya.

**05 — Command palette.** ⌘K over the empty-inbox screen. The input searches commands, people and mail, scoped to all accounts. Commands are grouped into Suggested, Go to, Settings and Accounts, and every row shows its shortcut. The last row is deliberately cut off by a fade so you can tell the list scrolls. Behind the palette is the empty-inbox screen: a penguin on an ice floe on a faint dot grid, a short message, and two actions (Review Other `L`, Undo last done `⌘Z`).

## Notes and deliberate choices

- The design is dark-first, and the light theme was tuned by hand rather than inverted. Row hover and selection, muted and faint text, the scrim and the search highlight all have their own light-mode values.
- Unread mail is shown with bold text plus a blue bar, never a dot alone. Selection is shown with a fill.
- The sidebar puts section labels on the Labels and Accounts groups, because a mail client needs that structure. The views list above them has no label.
- Every person, company, address and number is made up. All addresses use `.example` domains.
- To regenerate the screenshots, run headless Chrome at `--window-size=1440,900 --force-device-scale-factor=2 --virtual-time-budget=5000 --screenshot=… file://…/NN-name.html?theme=light|dark`.

## Additional MVP screens — v0.2

The gallery now includes ten screens. Screens 06–10 reuse the original tokens,
account colors, icon sprite, buttons, badges, key hints, and floating surfaces.
All added styles live in `penguin-screens-2.css`; the original five pages,
`penguin.css`, and `penguin.js` are unchanged.

| Page | What it shows |
|---|---|
| `06-welcome.html` | First launch with the Penguin mark and “Every email, instantly.” Two ordered steps: import the Google OAuth client JSON, then add accounts. The Google sign-in button is disabled until the client is connected. Links to `../docs/google-setup.md`. |
| `07-accounts-sync.html` | Northwind backfilling 38,112 of 142,318 messages, Harbor Labs in incremental sync, and Personal needing sign-in. Recent mail remains usable, search coverage grows during backfill, and continuing to the inbox does not require every account to finish. |
| `08-settings.html` | Two columns containing Accounts, General, Privacy, Search, AI, and Shortcuts. Account dots, reorder arrows, and removal controls sit beside each address. The local index shows 2.1 GB for 318,204 messages. Each AI feature is off by default; Local and Bring your own key make the processing choice explicit. Jev triage is labeled experimental. |
| `09-states.html` | Eight examples: inbox zero, no local search matches, offline, needs sign-in, undo-send, archived, send failed, and remote images blocked. The empty inbox reuses the original penguin on an ice floe. Search scope expansion and “Search Gmail too” are separate actions. Tracker blocking stays visible when images can be loaded. |
| `10-shortcuts.html` | A keyboard cheat sheet over the original inbox, with movement, triage, writing, search, and G-sequence navigation. Uses `J/K`, `O/Enter`, `E`, `#`, `R`, `A`, `F`, `C`, `/`, `mod+K`, `G I`, `G S`, `G T`, `G D`, `S`, `U`, `Shift+U`, `L`, `X`, `Z`, and `?`. On Mac, mod is Command. |

### Presentation behavior and implementation notes

These are static design references. Controls illustrate states; they do not
perform OAuth, read a JSON file or clipboard, remove accounts, rebuild a real
index, send mail, or enable AI. `penguin-screens-2.js` only carries the selected
theme through links, switches the settings theme, keeps the cheat-sheet inbox
backdrop in the same theme, and handles Escape on the cheat sheet. Press **T**
to toggle the theme, as in the original mockups.

The account fixtures remain Sam Okafor at Northwind, Harbor Labs, and Personal,
using `.example` email domains. Message examples retain Priya Natarajan and
Linden & Co. New product copy follows the MVP architecture: local message
search, newest-first sync, explicit remote search, and no Penguin server.
The optional AI controls are design proposals, not claims of implemented MVP
capabilities. The index path and mailbox sizes are illustrative fixtures.

This sheet uses **E = Archive**, **U = Back to list**, **Shift+U = Mark unread**,
**O/Enter = Open**, and **Z = Undo**. The earlier visual proofs use “Done” for
archive and contain some different key hints. Those original files remain
untouched; the new cheat sheet specifies the requested MVP vocabulary.

### Screenshot rendering

Run from the repository root:

```sh
python3 design/render-screens-2.py
```

The script calls `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
with `--headless=new --window-size=1440,900 --force-device-scale-factor=2
--virtual-time-budget=4000`, once per page and theme. It uses a temporary Chrome
profile under `design/`, removes it on completion, and checks that each PNG is
2880×1800 pixels before replacing its destination. It never modifies the
original ten screenshots.

Expected files in `screenshots/` are `06-welcome-{dark,light}.png`,
`07-accounts-sync-{dark,light}.png`, `08-settings-{dark,light}.png`,
`09-states-{dark,light}.png`, and `10-shortcuts-{dark,light}.png`.

Visual review is pending: the restricted design session could not launch
headless Chrome (exit 134 / SIGABRT), and connected-browser security policy
blocked local-file navigation. Rendering must succeed before screenshot
inspection and the two visual refinement passes can be completed.
