//! Pictures and attachments leaving the app as files (docs/SECURITY.md →
//! "Image viewer", "Drag out and Save As").
//!
//! 1. **Drag out.** A real file drag to Finder, the Desktop or another app
//!    needs a file on disk and a native `NSDraggingSession`; WKWebView's own
//!    drag of an `<img>` only carries a URL or a picture of the page. The UI
//!    asks for the file first ([`prepare_image_drag`],
//!    [`prepare_attachment_drag`]): the bytes come from the same checked
//!    paths as Save (a body picture's `data:` or public https source, sniffed
//!    as an image; an attachment's cached or fetched bytes) and are written
//!    to `<cache>/drag-out/<session>/<n>/<name>` with a sanitized name. Then
//!    [`start_file_drag`] starts the session with the `drag` crate
//!    (CrabNebula's drag-rs, the crate behind tauri-plugin-drag), and only
//!    for a path this session wrote there: the UI can't drag an arbitrary
//!    file out. At launch, files older than a day are removed ([`prune`]).
//! 2. **Save As.** The system save panel (rfd's NSSavePanel sheet on macOS),
//!    then the same bytes written where the user chose. The chosen path comes
//!    from the panel, never from the UI.
//! 3. **Show in Finder** for a file this session saved ([`reveal_saved_path`]).
//! 4. **Copy** an attachment as a file ([`copy_attachment_file`]): the bytes
//!    are written the same way as for a drag, then that file's URL goes on
//!    the general pasteboard as Finder's Copy puts it (src/mac/pasteboard.rs).
//!    Pasting in Finder, Slack, Mail or Penguin's composer gives the file.
//!    Browser clipboard APIs can't write file URLs, hence a command.
//!    Write-only: nothing on the pasteboard is read back.
//!
//! Drag out, Copy and Save As are macOS-only; elsewhere the commands answer
//! `invalidInput` and the UI hides them.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use image::{ImageFormat, ImageReader, Limits};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type ExportsRef<'a> = State<'a, Arc<Exports>>;

/// Under the cache dir; one subdirectory per app session.
pub const DIR: &str = "drag-out";
/// Drag-out files older than this are removed at launch. By then every drop
/// has long been copied by its destination.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest edge of the picture under the pointer while dragging, in points.
const PREVIEW_EDGE: u32 = 160;
/// Pictures larger than this aren't decoded for the drag preview (the file
/// is still dragged, with the generic page).
const MAX_PREVIEW_DIMENSION: u32 = 12_000;

/// A file written for a drag.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragFile {
    pub path: String,
    pub name: String,
}

/// This session's drag-out files and the preview drawn for each.
pub struct Exports {
    root: PathBuf,
    session: PathBuf,
    next: AtomicU32,
    files: Mutex<HashMap<PathBuf, Vec<u8>>>,
}

impl Exports {
    pub fn new(cache_dir: &Path) -> Self {
        let root = cache_dir.join(DIR);
        let session = root.join(session_id());
        Exports {
            root,
            session,
            next: AtomicU32::new(0),
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Write `bytes` as `<session>/<n>/<name>`: a directory per file keeps
    /// the name exactly as shown (no " (2)"), and `create_new` never follows
    /// or replaces anything already there.
    fn write(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        use std::io::Write;
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let dir = self.session.join(n.to_string());
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(bytes)?;
        Ok(path)
    }

    /// Write the file and its drag preview, and remember both.
    fn add(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.write(name, bytes)?;
        let preview = thumbnail(bytes).unwrap_or_else(document_glyph);
        self.files.lock().unwrap().insert(path.clone(), preview);
        Ok(path)
    }

    /// The preview of a file this session wrote for a drag, if `path` is one
    /// that is still there as a plain file. Nothing else can be dragged.
    fn preview_for(&self, path: &Path) -> Option<Vec<u8>> {
        if !path.starts_with(&self.session) {
            return None;
        }
        let preview = self.files.lock().unwrap().get(path).cloned()?;
        let meta = std::fs::symlink_metadata(path).ok()?;
        meta.file_type().is_file().then_some(preview)
    }
}

/// 64 random bits in hex, so two running copies never share a directory.
fn session_id() -> String {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_err() {
        // Still unique per process and launch.
        let t = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        buf = (t ^ u64::from(std::process::id())).to_le_bytes();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Remove drag-out files last modified more than `max_age` before `now`,
/// then the directories left empty. `keep` (this session's directory) is
/// never touched. Symlinks are removed, never followed. Returns the number
/// of files removed.
pub fn prune(root: &Path, keep: &Path, now: SystemTime, max_age: Duration) -> usize {
    fn walk(dir: &Path, keep: &Path, now: SystemTime, max_age: Duration, depth: u32) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path == keep {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() && depth < 4 {
                removed += walk(&path, keep, now, max_age, depth + 1);
                // Only succeeds once it's empty.
                let _ = std::fs::remove_dir(&path);
            } else if !meta.is_dir() {
                let old = meta
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .is_some_and(|age| age > max_age);
                if old && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
    walk(root, keep, now, max_age, 0)
}

/// At launch: remove earlier sessions' drag-out files older than a day.
pub fn spawn_prune(exports: Arc<Exports>) {
    tauri::async_runtime::spawn_blocking(move || {
        let n = prune(&exports.root, &exports.session, SystemTime::now(), MAX_AGE);
        if n > 0 {
            tracing::info!(removed = n, "removed old drag-out files");
        }
    });
}

/// A PNG of the picture, at most [`PREVIEW_EDGE`] on its longest side, or
/// None when the bytes aren't a raster the app decodes (the drag then shows
/// [`document_glyph`]). Decoded under limits; the preview is our own PNG.
fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let format = image::guess_format(bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Ico
    ) {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_PREVIEW_DIMENSION);
    limits.max_image_height = Some(MAX_PREVIEW_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let img = if img.width().max(img.height()) > PREVIEW_EDGE {
        img.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
    } else {
        img
    };
    let mut out = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// A plain page with a folded corner and a few lines: the drag preview for
/// a file that isn't a picture (a PDF, a document). Drawn here so the drag
/// never depends on decoding the file.
fn document_glyph() -> Vec<u8> {
    const W: u32 = 64;
    const H: u32 = 80;
    const FOLD: u32 = 16;
    let paper = image::Rgba([250, 250, 252, 255]);
    let edge = image::Rgba([160, 164, 172, 255]);
    let line = image::Rgba([200, 204, 212, 255]);
    let clear = image::Rgba([0, 0, 0, 0]);
    let img = image::RgbaImage::from_fn(W, H, |x, y| {
        // Cut off the top-right corner along the diagonal of the fold.
        let fold_x = W - FOLD;
        if x >= fold_x && y < FOLD {
            let (dx, dy) = (x - fold_x, y);
            return if dx > dy {
                clear
            } else if dx == dy || x == fold_x || y == FOLD - 1 {
                edge
            } else {
                paper
            };
        }
        if x == 0 || y == H - 1 || y == 0 || x == W - 1 {
            return edge;
        }
        let text = (12..W - 12).contains(&x) && y >= 28 && y < H - 12 && (y - 28) % 8 < 2;
        if text {
            line
        } else {
            paper
        }
    });
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .expect("encoding a PNG in memory can't fail");
    out
}

fn io_err(e: std::io::Error) -> CmdError {
    CmdError::other(format!("Couldn't prepare the file: {e}"))
}

fn drag_file(path: PathBuf) -> DragFile {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    DragFile {
        path: path.display().to_string(),
        name,
    }
}

/// A picture in a message body (the same sources and checks as
/// save_message_image) as a drag-out file.
#[tauri::command]
pub async fn prepare_image_drag(
    exports: ExportsRef<'_>,
    url: String,
    filename: String,
) -> CmdResult<DragFile> {
    let (bytes, mime) = crate::image_viewer::image_bytes(&url).await?;
    let name = crate::image_viewer::file_name(&filename, mime);
    let exports = exports.inner().clone();
    let path = blocking(move || exports.add(&name, &bytes).map_err(io_err)).await?;
    tracing::info!(kind = mime, "message image ready to drag");
    Ok(drag_file(path))
}

/// An attachment as a drag-out file under its own (sanitized) name.
#[tauri::command]
pub async fn prepare_attachment_drag(
    state: AppStateRef<'_>,
    exports: ExportsRef<'_>,
    account_id: String,
    message_id: String,
    attachment_id: String,
) -> CmdResult<DragFile> {
    let (attachment, bytes) =
        crate::commands::attachment_bytes(&state, &account_id, &message_id, &attachment_id).await?;
    let name = ops::sanitize_filename(&attachment.filename);
    let exports = exports.inner().clone();
    let path = blocking(move || exports.add(&name, &bytes).map_err(io_err)).await?;
    tracing::info!(account = %account_id, message = %message_id, "attachment ready to drag");
    Ok(drag_file(path))
}

/// Copy an attachment to the clipboard as a file (menu Copy, ⌘C on a
/// focused card). Returns the file written for it.
#[tauri::command]
pub async fn copy_attachment_file(
    app: AppHandle,
    state: AppStateRef<'_>,
    exports: ExportsRef<'_>,
    account_id: String,
    message_id: String,
    attachment_id: String,
) -> CmdResult<DragFile> {
    if !cfg!(target_os = "macos") {
        return Err(CmdError::invalid("Copying files works on macOS only"));
    }
    let (attachment, bytes) =
        crate::commands::attachment_bytes(&state, &account_id, &message_id, &attachment_id).await?;
    let name = ops::sanitize_filename(&attachment.filename);
    let exports = exports.inner().clone();
    let path = blocking(move || exports.write(&name, &bytes).map_err(io_err)).await?;
    put_on_pasteboard(&app, path.clone()).await?;
    tracing::info!(account = %account_id, message = %message_id, "attachment copied as a file");
    Ok(drag_file(path))
}

#[cfg(target_os = "macos")]
async fn put_on_pasteboard(app: &AppHandle, path: PathBuf) -> CmdResult<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(crate::mac::pasteboard::write_files(&[path.as_path()]));
    })
    .map_err(|e| CmdError::other(e.to_string()))?;
    match rx.await {
        Ok(true) => Ok(()),
        _ => Err(CmdError::other("The clipboard refused the file")),
    }
}

#[cfg(not(target_os = "macos"))]
async fn put_on_pasteboard(_app: &AppHandle, _path: PathBuf) -> CmdResult<()> {
    Err(CmdError::invalid("Copying files works on macOS only"))
}

/// Start a native file drag of a file [`prepare_image_drag`] or
/// [`prepare_attachment_drag`] wrote this session, from where the pointer
/// is. The UI calls it from `dragstart`, while the button is still down.
#[tauri::command]
pub async fn start_file_drag(
    app: AppHandle,
    window: tauri::Window,
    exports: ExportsRef<'_>,
    path: String,
) -> CmdResult<()> {
    let path = PathBuf::from(path);
    let preview = exports
        .preview_for(&path)
        .ok_or_else(|| CmdError::invalid("Only files Penguin prepared can be dragged"))?;
    native_drag(&app, window, path, preview).await
}

#[cfg(target_os = "macos")]
async fn native_drag(
    app: &AppHandle,
    window: tauri::Window,
    path: PathBuf,
    preview: Vec<u8>,
) -> CmdResult<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    // AppKit: the session must begin on the main thread. It returns at once;
    // the drag itself runs in the event loop.
    app.run_on_main_thread(move || {
        let started = drag::start_drag(
            &window,
            drag::DragItem::Files(vec![path]),
            drag::Image::Raw(preview),
            |result, _| tracing::debug!(?result, "file drag ended"),
            drag::Options::default(),
        );
        let _ = tx.send(started);
    })
    .map_err(|e| CmdError::other(e.to_string()))?;
    match rx.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "file drag failed to start");
            Err(CmdError::other(format!("Couldn't start the drag: {e}")))
        }
        Err(_) => Err(CmdError::other("Couldn't start the drag")),
    }
}

#[cfg(not(target_os = "macos"))]
async fn native_drag(
    _app: &AppHandle,
    _window: tauri::Window,
    _path: PathBuf,
    _preview: Vec<u8>,
) -> CmdResult<()> {
    Err(CmdError::invalid(
        "Dragging files out of Penguin works on macOS only",
    ))
}

/// The save panel, starting in Downloads with `name`; None when cancelled.
#[cfg(target_os = "macos")]
async fn ask_save_path(window: &tauri::Window, name: &str) -> Option<PathBuf> {
    let panel = {
        let mut d = rfd::AsyncFileDialog::new()
            .set_file_name(name)
            .set_parent(window);
        if let Some(dir) = dirs::download_dir() {
            d = d.set_directory(dir);
        }
        d.save_file()
    };
    panel.await.map(|f| f.path().to_path_buf())
}

#[cfg(not(target_os = "macos"))]
async fn ask_save_path(_window: &tauri::Window, _name: &str) -> Option<PathBuf> {
    None
}

/// Write the bytes where the panel said (it already confirmed replacing an
/// existing file) and remember the path for Open / Show in Finder.
async fn save_as(
    state: &AppState,
    window: &tauri::Window,
    name: &str,
    bytes: Vec<u8>,
) -> CmdResult<Option<String>> {
    if !cfg!(target_os = "macos") {
        return Err(CmdError::invalid("Save As works on macOS only"));
    }
    let Some(path) = ask_save_path(window, name).await else {
        return Ok(None);
    };
    let target = path.clone();
    blocking(move || {
        std::fs::write(&target, &bytes)
            .map_err(|e| CmdError::other(format!("Couldn't write the file: {e}")))
    })
    .await?;
    state.remember_saved_path(path.clone());
    Ok(Some(path.display().to_string()))
}

/// Save As… for a picture in a message body. Null when cancelled.
#[tauri::command]
pub async fn save_image_as(
    state: AppStateRef<'_>,
    window: tauri::Window,
    url: String,
    filename: String,
) -> CmdResult<Option<String>> {
    let (bytes, mime) = crate::image_viewer::image_bytes(&url).await?;
    let name = crate::image_viewer::file_name(&filename, mime);
    let saved = save_as(&state, &window, &name, bytes).await?;
    if saved.is_some() {
        tracing::info!(kind = mime, "message image saved (Save As)");
    }
    Ok(saved)
}

/// Save As… for an attachment. Null when cancelled.
#[tauri::command]
pub async fn save_attachment_as(
    state: AppStateRef<'_>,
    window: tauri::Window,
    account_id: String,
    message_id: String,
    attachment_id: String,
) -> CmdResult<Option<String>> {
    let (attachment, bytes) =
        crate::commands::attachment_bytes(&state, &account_id, &message_id, &attachment_id).await?;
    let name = ops::sanitize_filename(&attachment.filename);
    let saved = save_as(&state, &window, &name, bytes).await?;
    if saved.is_some() {
        tracing::info!(account = %account_id, message = %message_id, "attachment saved (Save As)");
    }
    Ok(saved)
}

/// Show in Finder: only a file this session saved (like open_path).
#[tauri::command]
pub async fn reveal_saved_path(
    app: AppHandle,
    state: AppStateRef<'_>,
    path: String,
) -> CmdResult<()> {
    let path = PathBuf::from(path);
    if !state.is_saved_path(&path) {
        return Err(CmdError::invalid(
            "Only files saved by Penguin can be shown",
        ));
    }
    if !path.exists() {
        return Err(CmdError::not_found("That file was moved or deleted"));
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| CmdError::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("penguin-export-{tag}-{}", session_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 120, 200, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn files_keep_their_name_in_a_directory_each() {
        let cache = tmp("names");
        let ex = Exports::new(&cache);
        let a = ex.add("hero.png", &png(4, 4)).unwrap();
        let b = ex.add("hero.png", &png(4, 4)).unwrap();
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert_eq!(p.file_name().unwrap(), "hero.png");
            assert!(p.starts_with(cache.join(DIR)));
            assert!(p.starts_with(&ex.session));
        }
        assert_eq!(ex.session.parent().unwrap(), cache.join(DIR));
        assert_eq!(ex.session.file_name().unwrap().len(), 16);
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn only_files_this_session_wrote_can_be_dragged() {
        let cache = tmp("only");
        let ex = Exports::new(&cache);
        let ok = ex.add("a.png", &png(4, 4)).unwrap();
        assert!(ex.preview_for(&ok).is_some());
        // Same directory, not written by add.
        let stray = ok.with_file_name("b.png");
        std::fs::write(&stray, png(4, 4)).unwrap();
        assert!(ex.preview_for(&stray).is_none());
        // Outside the session, or a path that walks out of it.
        assert!(ex.preview_for(Path::new("/etc/passwd")).is_none());
        assert!(ex.preview_for(&ex.session.join("../x")).is_none());
        // Another Exports (another session) doesn't know it.
        assert!(Exports::new(&cache).preview_for(&ok).is_none());
        // Replaced by something that isn't a plain file.
        std::fs::remove_file(&ok).unwrap();
        std::fs::create_dir(&ok).unwrap();
        assert!(ex.preview_for(&ok).is_none());
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn previews_are_small_pngs_or_the_page_glyph() {
        let big = thumbnail(&png(800, 400)).unwrap();
        let img = image::load_from_memory_with_format(&big, ImageFormat::Png).unwrap();
        assert_eq!((img.width(), img.height()), (160, 80));
        let small = thumbnail(&png(20, 30)).unwrap();
        let img = image::load_from_memory(&small).unwrap();
        assert_eq!((img.width(), img.height()), (20, 30));
        assert!(thumbnail(b"%PDF-1.7 not a picture").is_none());
        assert!(thumbnail(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
        let glyph = image::load_from_memory(&document_glyph()).unwrap();
        assert_eq!((glyph.width(), glyph.height()), (64, 80));
        // A PDF gets the glyph.
        let cache = tmp("pdf");
        let ex = Exports::new(&cache);
        let p = ex.add("Brief.pdf", b"%PDF-1.7 ...").unwrap();
        assert_eq!(ex.preview_for(&p).unwrap(), document_glyph());
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn prune_removes_old_files_and_empty_dirs_but_not_this_session() {
        let cache = tmp("prune");
        let root = cache.join(DIR);
        let old_session = root.join("00000000000000aa");
        std::fs::create_dir_all(old_session.join("0")).unwrap();
        std::fs::create_dir_all(old_session.join("1")).unwrap();
        std::fs::write(old_session.join("0/a.png"), b"x").unwrap();
        std::fs::write(old_session.join("1/b.pdf"), b"y").unwrap();
        let ex = Exports::new(&cache);
        let mine = ex.add("mine.png", &png(2, 2)).unwrap();

        // Nothing is a day old yet.
        assert_eq!(prune(&root, &ex.session, SystemTime::now(), MAX_AGE), 0);
        assert!(old_session.join("0/a.png").exists());

        // Two days later: the old session goes, file and directories; this
        // session's file stays even though it's just as old.
        let later = SystemTime::now() + Duration::from_secs(2 * 24 * 60 * 60);
        assert_eq!(prune(&root, &ex.session, later, MAX_AGE), 2);
        assert!(!old_session.exists());
        assert!(mine.exists());
        assert!(root.exists());
        // A missing root is fine.
        assert_eq!(prune(&cache.join("nope"), &ex.session, later, MAX_AGE), 0);
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn prune_never_follows_symlinks() {
        let cache = tmp("links");
        let root = cache.join(DIR);
        let outside = cache.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"k").unwrap();
        std::fs::create_dir_all(root.join("s")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("s/link")).unwrap();
        let later = SystemTime::now() + Duration::from_secs(2 * 24 * 60 * 60);
        prune(&root, &root.join("current"), later, MAX_AGE);
        assert!(outside.join("keep.txt").exists());
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn file_names_are_sanitized_and_follow_the_bytes() {
        use crate::image_viewer::file_name;
        assert_eq!(file_name("../../hero.jpg", "image/png"), "_.._hero.png");
        assert_eq!(ops::sanitize_filename("a/b\\c:d.pdf"), "a_b_c_d.pdf");
        let cache = tmp("sanitize");
        let ex = Exports::new(&cache);
        let p = ex
            .add(&ops::sanitize_filename("../escape.pdf"), b"%PDF-")
            .unwrap();
        assert!(p.starts_with(&ex.session));
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
