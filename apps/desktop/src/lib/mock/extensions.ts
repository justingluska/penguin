// Mock extensions (features/extensions): one fictional "Tasks" extension with
// forms, as if installed in the extensions folder, plus a folder that isn't
// a valid extension. Running an action answers like a program would: "Close
// task" asks to confirm first (with choices), then closes; a comment with
// "fail" in it fails, to show the sheet's error.
import type { ExtensionInfo, ExtensionsOverview, ExtensionResult, RunExtensionRequest } from "../types";
import type { MockHandler } from "./index";

const TASKS_ICON =
  "data:image/svg+xml;base64," +
  btoa(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#6d5bd0" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="4"/><path d="m8 12 3 3 5-6"/></svg>',
  );

const extensions: ExtensionInfo[] = [
  {
    id: "tasks",
    name: "Tasks",
    description: "Turn an email into a task on your team's board.",
    folder: "tasks",
    icon: null,
    iconSvg: TASKS_ICON,
    state: "on",
    actions: [
      {
        id: "create",
        title: "Create task",
        icon: null,
        iconSvg: TASKS_ICON,
        includeBody: true,
        includeThread: false,
        command: ["/usr/bin/osascript", "-l", "JavaScript", "create-task.js"],
        form: [
          { id: "title", label: "Title", type: "text", default: "{{subject}}", placeholder: null, options: [], required: true },
          {
            id: "board",
            label: "Board",
            type: "select",
            default: "ops",
            placeholder: null,
            options: [
              { value: "ops", label: "Operations" },
              { value: "web", label: "Website" },
            ],
            required: false,
          },
          { id: "due", label: "Due", type: "text", default: "", placeholder: "today, tomorrow, +3d, 2026-11-02", options: [], required: false },
          { id: "notes", label: "Notes", type: "textarea", default: "", placeholder: "Anything to add", options: [], required: false },
          { id: "includeEmail", label: "Include the email's text", type: "checkbox", default: "true", placeholder: null, options: [], required: false },
        ],
      },
      {
        id: "close",
        title: "Close task",
        icon: "squarecheck",
        iconSvg: null,
        includeBody: false,
        includeThread: true,
        command: ["/usr/bin/osascript", "-l", "JavaScript", "close-task.js"],
        form: [{ id: "comment", label: "Comment", type: "textarea", default: "", placeholder: "Why it's closing, posted on the task", options: [], required: true }],
      },
    ],
  },
  {
    id: "save-to-notes",
    name: "Save to notes",
    description: "Append the email to a notes file.",
    folder: "save-to-notes",
    icon: "book",
    iconSvg: null,
    state: "off",
    actions: [{ id: "save", title: "Save to notes", icon: "book", iconSvg: null, includeBody: false, includeThread: true, command: ["save.sh"], form: [] }],
  },
];

const overview = (): ExtensionsOverview => ({
  dir: "~/Library/Application Support/Penguin/extensions",
  extensions: extensions.map((e) => ({ ...e })),
  problems: [{ folder: "half-written", error: "extension.json: expected value at line 1 column 3" }],
});

let taskNo = 41;

export const extensionHandlers: Record<string, MockHandler> = {
  list_extensions: () => overview(),
  set_extension_allowed: ({ id, allowed }) => {
    const e = extensions.find((x) => x.id === id);
    if (!e) throw { code: "notFound", message: `no extension "${id}"` };
    e.state = allowed ? "on" : "off";
    return overview();
  },
  run_extension_action: async (args): Promise<ExtensionResult> => {
    const request = args.request as RunExtensionRequest;
    const e = extensions.find((x) => x.id === request.extensionId);
    if (!e || e.state !== "on") throw { code: "permissionDenied", message: `${e?.name ?? "That extension"} isn't allowed yet. Turn it on in Settings → Extensions.` };
    await new Promise((r) => setTimeout(r, 600));
    if (e.id === "tasks" && request.actionId === "close") {
      const comment = request.form.comment ?? "";
      if (/fail/i.test(comment)) throw { code: "other", message: "Tasks: the board answered 503 Service Unavailable. Try again in a minute." };
      if (!request.continuation) {
        return {
          message: null,
          url: null,
          confirm: {
            title: "Close a task with a comment",
            detail: "Two open tasks match this conversation",
            steps: [
              { text: `Comment: “${comment.length > 80 ? comment.slice(0, 80) + "…" : comment}”`, url: null },
              { text: "Move the task to Done", url: null },
              { text: "Link this conversation on the task", url: "https://tasks.example/board/ops" },
            ],
            choices: [
              { value: "41", label: "Northwind blog brief", detail: "OPS-41 · To do · Riley" },
              { value: "57", label: "Harbor Labs landing page copy", detail: "OPS-57 · In review · Priya" },
            ],
            choiceLabel: "Which task?",
            confirmLabel: "Close task",
            state: JSON.stringify({ comment: comment.length }),
          },
        };
      }
      const id = request.continuation.choice ?? "41";
      return { message: `Closed OPS-${id} with your comment`, url: `https://tasks.example/OPS-${id}`, confirm: null };
    }
    if (e.id === "tasks") {
      taskNo += 1;
      return { message: `Created OPS-${taskNo}: ${request.form.title}`, url: `https://tasks.example/OPS-${taskNo}`, confirm: null };
    }
    return { message: "Saved", url: null, confirm: null };
  },
  open_extensions_folder: () => undefined,
};
