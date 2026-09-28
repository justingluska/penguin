// Copy text from a menu or button, with a confirming toast. WebKit allows
// clipboard writes inside a user gesture (a click or a menu choice).
import { toast } from "../components/Toast";

export async function copyText(text: string, done = "Copied"): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast({ message: done });
  } catch (e) {
    toast({ tone: "error", message: "Couldn't copy", detail: String((e as Error)?.message ?? e) });
  }
}

/**
 * Copy text that's still being gathered. WebKit only allows a clipboard
 * write during the click itself, so the write starts now with a promised
 * value (ClipboardItem accepts one) and falls back to a plain write after.
 */
export async function copyTextLater(text: Promise<string>, done = "Copied"): Promise<void> {
  try {
    if (typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
      const blob = text.then((t) => new Blob([t], { type: "text/plain" }));
      await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
      toast({ message: done });
      return;
    }
  } catch {
    // Some engines refuse a promised item; the plain write below may still work.
  }
  let value: string;
  try {
    value = await text;
  } catch (e) {
    // Gathering the text failed (the thing to copy is gone): say so, don't leave a stray rejection.
    toast({ tone: "error", message: "Couldn't copy", detail: String((e as Error)?.message ?? e) });
    return;
  }
  await copyText(value, done);
}
