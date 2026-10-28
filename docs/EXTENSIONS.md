# Extensions

An extension adds buttons to your emails that run a program on your Mac, such as "Create task", "Save to notes" or "Send to my CRM". Penguin doesn't know what the program does. The extension is a folder you install, so personal or company-specific integrations stay out of Penguin itself.

Code: `apps/desktop/src-tauri/src/extensions.rs` (tests in `extensions_tests.rs`), UI in `apps/desktop/src/features/extensions/`, payload schema in `docs/cli-schemas/extensionAction.v1.schema.json`, confirm answer schema in `docs/cli-schemas/extensionConfirm.v1.schema.json`.

## Installing one

1. Open Settings → Extensions → **Open extensions folder**. It's `extensions/` inside Penguin's config folder.
2. Put the extension's folder there. A symlink to a checkout of its repo works too.
3. Back in Penguin (the list reloads when the window comes to the front, or click **Reload**), turn on **Allow** for it. Settings shows exactly which command each button runs and whether it receives the email's text or the whole conversation (every message in it, with their text).

Each allowed action then shows up in three places:

- as an icon button in every message's header, next to Reply and Forward;
- in the message's right-click menu;
- in ⌘K, where it acts on the selected conversation's latest message.

**Approval covers the folder exactly as it was.** Penguin takes a SHA-256 over every file in the folder when you allow it, skipping dot-folders such as `.git`, and checks it again right before each run. If you edit the script or the manifest, or add a file, the extension turns off and shows "changed" until you allow it again.

## extension.json

```json
{
  "penguinExtension": 1,
  "id": "tasks",
  "name": "Tasks",
  "description": "Turn an email into a task on your team's board.",
  "icon": "icon.svg",
  "actions": [
    {
      "id": "create",
      "title": "Create task",
      "program": "/usr/bin/osascript",
      "args": ["-l", "JavaScript", "create-task.js"],
      "includeBody": true,
      "timeoutSecs": 30,
      "form": [
        { "id": "title", "label": "Title", "default": "{{subject}}", "required": true },
        { "id": "board", "label": "Board", "type": "select",
          "options": [{ "value": "ops", "label": "Operations" }, { "value": "web", "label": "Website" }] },
        { "id": "notes", "label": "Notes", "type": "textarea", "placeholder": "Anything to add" },
        { "id": "includeEmail", "label": "Include the email's text", "type": "checkbox", "default": "true" }
      ]
    }
  ]
}
```

| Field | |
|---|---|
| `penguinExtension` | Always `1`. A newer version is refused, not half-read. |
| `id` | 1–40 lowercase letters, digits or dashes; unique across folders. |
| `name`, `description` | Shown in Settings, menus and toasts. |
| `icon` | Either a `.svg` file in the folder (at most 32 KB; drawn as an image, so it can't run anything) or the name of one of Penguin's built-in icons (`zap`, `book`, `squarecheck`…). An action without its own `icon` uses the extension's. |
| `actions[]` | 1–10 actions. |
| `actions[].program` | An absolute path, or a path inside the extension's folder (no `..`). It must be executable. |
| `actions[].args` | Passed exactly as written. No shell is involved, and no email content ever goes into the arguments. |
| `actions[].includeBody` | `true` to receive the message's text (with quoted history cut, at most 100,000 characters). Off by default. |
| `actions[].includeThread` | `true` to receive the whole conversation as `thread`: every message in it, oldest first, each with its text (quoted history cut). At most 100,000 characters of text in all: when it doesn't fit, the oldest messages are left out and `thread.truncated` is true. Off by default; independent of `includeBody`, which is about `message.text`. |
| `actions[].timeoutSecs` | 1–120, default 30. The program is killed after that. |
| `actions[].form[]` | Up to 12 fields Penguin asks for before running. `type` is `text` (default), `textarea`, `select` (with `options`) or `checkbox`. `default` may use `{{subject}}`, `{{from.name}}`, `{{from.email}}` and `{{snippet}}`. `required` blocks the run button until the field is filled. A field's `id` is the key your program reads: letters, digits, `-` or `_`, so camelCase is fine. |

Unknown fields are an error, so a typo doesn't get silently ignored. A folder that doesn't load is listed under "Not loaded" in Settings, with the reason. To check a folder against these rules from a checkout of Penguin, run `PENGUIN_CHECK_EXTENSION=/path/to/folder cargo test -p penguin-desktop --lib check_extension_folder -- --nocapture`.

## What the program gets

- **Working directory:** the extension's folder.
- **Environment:** the same minimal set the rule hooks get. `PATH` is `/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin`, plus `HOME`, `USER`, `LOGNAME`, `LANG` and `TMPDIR`. On top of that come `PENGUIN_EXTENSION_ID`, `PENGUIN_ACTION_ID` and `PENGUIN_ACCOUNT` (the account's address).
- **stdin:** one JSON document (`extensionAction`, schema version 1):

```json
{
  "schemaVersion": 1,
  "kind": "extensionAction",
  "data": {
    "extension": { "id": "tasks", "name": "Tasks" },
    "action": { "id": "create", "name": "Create task" },
    "accountId": "sam@northwind.example",
    "accountEmail": "sam@northwind.example",
    "threadId": "18c2f0a1b2",
    "includesBody": true,
    "includesThread": true,
    "message": { "id": "…", "date": 1730000000000, "dateIso": "…", "from": { "name": "Priya", "email": "priya@harbor.example" },
                 "to": [], "cc": [], "bcc": [], "subject": "…", "labelIds": [], "unread": false, "starred": false,
                 "attachments": [], "inlineImages": [], "text": "…", "quotedChars": 0, "fullText": null,
                 "truncated": false, "bodyPending": false },
    "thread": {
      "messages": [
        { "id": "…", "dateIso": "…", "from": { "name": "Priya", "email": "priya@harbor.example" },
          "to": [], "cc": [], "subject": "…", "text": "…", "truncated": false, "bodyPending": false }
      ],
      "truncated": false
    },
    "webUrl": "https://mail.google.com/mail/u/sam@northwind.example/#all/18c2f0a1b2",
    "form": { "title": "…", "board": "ops", "notes": "", "includeEmail": "true" },
    "continuation": { "state": "…", "choice": "41" }
  }
}
```

`message` uses the same shape as `penguin-cli thread --json` (see CLI.md). `text` is empty unless the action has `includeBody`. `thread` is there only when the action has `includeThread` (`includesThread` says which); the clicked message is one of its messages. A message stored with headers only (older than the sync window) has its body downloaded before the program runs; if that fails (offline, say), it still comes along with empty `text` and `bodyPending: true`. `webUrl` links to the conversation in Gmail and is null for other providers. Form answers are strings, and a checkbox is `"true"` or `"false"`. `continuation` is there only on a run after the person confirmed (see [Asking before acting](#asking-before-acting)). Penguin checks the answers against the manifest before it runs anything: only declared fields, and a select's value must be one of its options.

## What the program answers

- **Exit status 0** means it worked. Print either
  - `{"message": "Created OPS-42", "url": "https://…"}`: the toast shows the message, plus an **Open** button for the link (http or https only), or
  - a line of plain text, which becomes the toast.

  Penguin reads at most 64 KB of stdout and shows at most 300 characters.
  - `{"confirm": {...}}`: Penguin asks the person first and runs the action again when they confirm. See below.
- **Any other exit status** means it failed. The toast shows the extension's name and the first 300 characters of stderr. When the action has a form, the error shows in the form instead, so the person can fix an answer and run it again.

Penguin logs only the extension and action ids and whether the run succeeded, never the email, the answer or a confirm's `state`.

## Asking before acting

When the program needs the person to check something first (which of several matching tasks to close, say), it exits 0 and prints a confirm instead of acting:

```json
{"confirm": {
  "title": "Close FOR-123 with a comment",
  "detail": "On the Northwind board",
  "steps": [
    { "text": "Comment on FOR-123: “Lost the deal.”", "url": "https://tasks.example/FOR-123" },
    { "text": "Move FOR-123 to Done" }
  ],
  "choices": [
    { "value": "41", "label": "Northwind blog brief", "detail": "#41 · todo · Riley" },
    { "value": "57", "label": "Harbor landing page" }
  ],
  "choiceLabel": "Which task?",
  "confirmLabel": "Close task",
  "state": "{\"comment\":\"Lost the deal.\",\"nonce\":\"8f2c\"}"
}}
```

Penguin shows it in its own sheet: the title with `detail` under it, the steps as a numbered list (a step with a `url` gets an open-link button), the choices as a list to pick from under `choiceLabel`, and Cancel beside a button labelled `confirmLabel` (default "Continue"). Return confirms and Esc cancels. With `choices`, the person has to pick one before confirming (↑↓ or a click); a single choice comes preselected.

Confirming runs **the same action again** with the same payload (the same message and form answers), plus:

```json
"continuation": { "state": "<state exactly as you returned it>", "choice": "41" }
```

`choice` is the picked `value`, or null when there were no choices; `state` is null when you didn't return one. That run answers like any other: a message and link, plain text, a failure, or another confirm. Cancel runs nothing.

| Field | |
|---|---|
| `title` | Required. At most 200 characters. |
| `detail` | One line, at most 300 characters. |
| `steps[]` | At most 12. `text` at most 300 characters; `url` must be `http` or `https`. |
| `choices[]` | Optional, at most 20. `value` (unique, at most 200 characters) comes back as `continuation.choice`; `label` is shown, with `detail` muted under it. |
| `choiceLabel` | The question above the choices. Default "Choose one". |
| `confirmLabel` | The confirm button. At most 40 characters. |
| `state` | Opaque to Penguin, at most 8 KB. Use it to carry what you looked up, so the second run doesn't have to look again. |

The confirm is checked as strictly as the manifest: an unknown field (also beside `confirm`), a missing `title` or a limit broken is an error, shown with the reason. Each run has the action's own `timeoutSecs`. A click can confirm at most **3** times; a fourth confirm is an error.

`state` travels through Penguin's UI and back to your program on this Mac, but treat it like any input: check that it parses, and that `choice` is one you offered.

## Writing one without dependencies

macOS always has `/usr/bin/osascript -l JavaScript` (JXA), and its Objective-C bridge can read stdin and run `curl` with an argument list, so an extension can work without installing Node or Python. Any language works if it's on the Mac. Read secrets the way any script on the Mac would: from the Keychain (`/usr/bin/security find-generic-password -s <service> -w`), never from the manifest.

## A complete example

The smallest useful extension: one button that copies the email's subject and Gmail link to the clipboard. Two files in a folder named `copy-link`:

`extension.json`

```json
{
  "penguinExtension": 1,
  "id": "copy-link",
  "name": "Copy link",
  "description": "Copy the subject and a link to the conversation.",
  "icon": "link",
  "actions": [
    { "id": "copy", "title": "Copy link", "program": "/usr/bin/osascript", "args": ["-l", "JavaScript", "copy.js"] }
  ]
}
```

`copy.js`

```js
function run() {
  ObjC.import("Foundation");
  const stdin = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
  const { data } = JSON.parse($.NSString.alloc.initWithDataEncoding(stdin, $.NSUTF8StringEncoding).js);
  const text = data.webUrl ? `${data.message.subject}\n${data.webUrl}` : data.message.subject;
  const app = Application.currentApplication();
  app.includeStandardAdditions = true;
  app.setTheClipboardTo(text);
  return JSON.stringify({ message: "Copied" });
}
```

A JXA script's `run()` return value is what it prints, so returning the JSON is the answer. To fail, write to stderr and exit non-zero (`$.exit(1)` after `ObjC.import("stdlib")`).

## Building one with an agent

Extensions are a good job for a coding agent: the contract is this page and nothing else. Give the agent this page (or its URL) and what the button should do, plus these rules:

- **Work in the extension's own folder or repo, never in Penguin's source.** Penguin doesn't change to support one integration. If the manifest truly can't express something, that's a generic change to propose separately.
- **Keep the logic testable away from the Mac.** Put the part that turns the stdin payload into a request (or a file, or a note) in pure functions, and test them with sample payloads in the shape shown above. Keep stdin, the Keychain and the network in a thin layer at the bottom.
- **Secrets go in the Keychain**, put there by an install script (`security add-generic-password -U -s <service> -a "$USER" -w` prompts without echoing). Never in the manifest, an argument, a log or a committed file. Pass a token to `curl` on stdin (`-H @-`), not in its argument list, where other processes can see it.
- **Fail with a sentence the person can act on** ("No token in the Keychain: run install.sh"), since stderr becomes the toast.
- **Check the folder against Penguin's loader** before handing it over: `PENGUIN_CHECK_EXTENSION=/path/to/folder cargo test -p penguin-desktop --lib check_extension_folder -- --nocapture` from a Penguin checkout prints `ok: …` or the exact problem.
- **Then try it on a Mac:** install it (a symlink from the extensions folder to the checkout is easiest), allow it in Settings → Extensions, and run it on a real email. To test the program without Penguin, pipe a saved payload into it: `/usr/bin/osascript -l JavaScript copy.js < sample.json`.

Remember that any edit to the folder turns the extension off until it's allowed again, so a `git pull` or a script change shows up as "changed" in Settings. That's on purpose.
