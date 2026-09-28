# Demo mode

Demo mode runs the real Penguin app on fictional data, so you can take screenshots of any screen without showing your own mail. Every account, thread, person, calendar event, rule and setting you see comes from the built-in mock backend (`apps/desktop/src/lib/mock/`), which uses fictional people and `.example` addresses only.

## Turning it on and off

- **Settings → Developer → Demo mode**. Penguin saves any open drafts and reloads onto the demo data.
- **⌘K → "Enter demo mode"**.

To leave, use the same switch (Settings → Developer still shows it in demo mode) or **⌘K → "Exit demo mode"**. Penguin reloads onto your real mail.

The mode is saved per device (localStorage key `penguin.demoMode`), so it survives a restart until you turn it off. If a message is counting down to send (Undo send), Penguin waits for it and asks you to try again.

Nothing on screen marks demo mode, so screenshots stay clean. To check which mode you're in, look at the switch, the ⌘K command ("Exit demo mode" means you are in it), or in devtools the page title `Penguin (demo)` and `<html data-demo>`.

## What stays untouched

- **Every command and event goes to the mock.** `src/lib/api.ts` routes all calls to the mock backend, as `npm run dev:mock` does. Penguin doesn't ask the Rust side for mail, accounts, settings or the log, and nothing you do is sent, archived, labeled or saved to your accounts. Sending, snoozing, rules, unsubscribing, RSVPs and settings changes all happen in the mock's memory and are gone after a reload.
- **The page's local storage is swapped for an empty in-memory copy.** Your recent and saved searches, snippets, sidebar layout, calendar view and active profile never appear in demo mode, and anything you change there doesn't overwrite them.
- **The only real calls are window chrome.** The native menu bar still works (it reports which items are enabled and forwards menu clicks), and the window still hides with ⌘W.

The Rust side keeps running in the background while you're in demo mode: your accounts keep syncing, scheduled sends go out on time and rules keep running. Its macOS notifications (for example a snooze waking while the window is in the background) can still appear, so avoid screenshotting the notification area. Nothing real is shown inside the window.

## Taking screenshots

- The demo data uses dates relative to now ("Today", "Tomorrow", the calendar's current week), so every screen looks current whenever you take the shot.
- The theme follows macOS until you pick one (Settings → General or ⌘K → "Switch theme"). Demo settings changes last until the next reload.
- The sync progress in the sidebar and status bar is simulated: an account backfills for a few minutes after each reload. Wait for it to finish or include it on purpose.
- Every surface has demo data: inbox and All Inboxes, threads with attachments, verification-code chips and invites, search with operators and facets, Ask, Calendar (Agenda, Week, Month), compose, snooze, rules, person cards, unsubscribe and tracker dialogs, and Settings, including four fictional accounts with photos.
- In a browser (`npm run dev:mock`, or `npm run build` and `npx vite preview`) the URL can open a thread straight away: `?open=<accountId>/<threadId>` with `&att=<n>`, `&person=1` or `&details=1`. See `App.tsx` for the options.

## Cost

The mock ships in the production build as a separate chunk (`assets/mock-*.js`, about 166 KB, 59 KB gzipped). It's loaded only in demo mode, so normal use doesn't download or run it.
