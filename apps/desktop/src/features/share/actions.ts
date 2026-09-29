// "Copy Share Link" (docs/SHARE-LINKS.md): upload a picture or attachment
// to the user's own storage, copy a link that expires, and offer "Delete
// now". Before storage is set up it opens Settings → Share links with the
// file waiting, and finishes the share once setup and its test succeed.
import { api, asCommandError } from "../../lib/api";
import { dismissToast, toast } from "../../components/Toast";
import { busSend, isMainWindow } from "../../lib/windowBus";
import { MAIN_LABEL } from "../../lib/windowRoute";
import { focusMainWindow } from "../../lib/windowChrome";
import { openSettings } from "../settings/state";
import { bytes } from "../../lib/format";
import { expiresIn, type ShareTarget } from "./model";
import { refreshShareStatus, setPendingShare, setShareConfig } from "./state";

/** Uploads at least this big (or of unknown size) show a progress toast. */
const PROGRESS_FROM = 1024 * 1024;

// The menus read the status synchronously: load it once this module is used.
if (typeof window !== "undefined") void refreshShareStatus();

/** Settings → Share links, with `target` waiting to be shared (main window only has Settings). */
export function openShareSetup(target: ShareTarget | null) {
  if (!isMainWindow) {
    void busSend(MAIN_LABEL, "open-settings", { section: "sharing", share: target });
    return;
  }
  setPendingShare(target);
  void focusMainWindow();
  openSettings("sharing");
}

/**
 * The menu item. Reads the settings fresh (the menu's label may be stale
 * in this window): not set up → setup; else upload and copy.
 */
export async function copyShareLink(target: ShareTarget): Promise<void> {
  try {
    const c = await api.shareLinkConfigGet();
    setShareConfig(c);
    if (!c.configured) {
      openShareSetup(target);
      return;
    }
  } catch (e) {
    toast({ tone: "error", message: "Couldn't read the share-link settings", detail: asCommandError(e).message });
    return;
  }
  await shareNow(target);
}

/** Upload, copy the link, and say so (with "Delete now"). */
export async function shareNow(target: ShareTarget): Promise<boolean> {
  const big = target.size == null || target.size >= PROGRESS_FROM;
  const progress = big
    ? toast({
        kind: "progress",
        key: `share:${target.request.kind}:${target.name}`,
        message: `Uploading ${target.name}…`,
        detail: target.size != null ? bytes(target.size) : undefined,
      })
    : 0;
  try {
    const link = await api.shareFile(target.request);
    // Native pasteboard: the upload outlived the click that asked for it.
    const copied = await api.copyText(link.url).then(
      () => true,
      () => false,
    );
    const done = {
      kind: copied ? ("action" as const) : ("error" as const),
      message: copied ? `Link copied (${expiresIn(link.expiresAt, Date.now())})` : "Uploaded, but the clipboard refused the link",
      detail: link.name,
      duration: 10_000,
      actions: [{ label: "Delete now", run: () => void deleteShared(link.key, link.name) }],
    };
    // A fresh toast (the progress one may have been dismissed meanwhile).
    if (progress) dismissToast(progress);
    toast(done);
    return true;
  } catch (e) {
    const err = asCommandError(e);
    if (err.code === "notConfigured") {
      if (progress) dismissToast(progress);
      void refreshShareStatus();
      openShareSetup(target);
      return false;
    }
    const failed = { kind: "error" as const, message: `Couldn't share ${target.name}`, detail: err.message, duration: null };
    if (progress) dismissToast(progress);
    toast(failed);
    return false;
  }
}

async function deleteShared(key: string, name: string) {
  try {
    await api.shareDelete(key);
    toast({ kind: "success", message: `Deleted ${name} from your storage`, detail: "The link no longer works." });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't delete ${name}`, detail: asCommandError(e).message });
  }
}
