//! Extensions: buttons on a message that run a program the user installed.
//!
//! An extension is a folder in `<config dir>/extensions/` with an
//! `extension.json` manifest (version 1, documented in docs/EXTENSIONS.md):
//! a name, an icon and one or more actions, each a program with arguments,
//! an optional small form Penguin shows first, and whether the message's
//! text (or the whole conversation's) goes along. Penguin itself knows nothing about what an extension
//! does; private integrations live in their own folders, not in this repo.
//!
//! Nothing runs until the user allows the extension in Settings →
//! Extensions. The approval is for the folder's exact contents: a SHA-256
//! over every file in it, checked again right before each run, so an edited
//! script or manifest has to be allowed again.
//!
//! Running follows the rule hooks' model (rules/hooks.rs): the program and
//! its arguments are exactly what the manifest says, the environment is the
//! hooks' minimal base, and everything about the message arrives as JSON on
//! stdin (`extensionAction` v1, docs/cli-schemas). The program answers on
//! stdout: `{"message": "...", "url": "https://..."}` or a line of text, or
//! `{"confirm": {...}}` to have Penguin ask the person first; confirming
//! runs the same action again with `continuation` in the payload (at most
//! `MAX_CONFIRM_ROUNDS` times per click). Nothing about a message, and no
//! confirm state, is logged; ids and outcomes only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use penguin_core::{AccountProvider, Message, ThreadDetail};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::output::{self, AddressOut, Envelope, MessageOut, ThreadOptions};
use crate::error::{CmdError, CmdResult};
use crate::rules::hooks;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = tauri::State<'a, Arc<AppState>>;

pub const DIR_NAME: &str = "extensions";
pub const MANIFEST: &str = "extension.json";
/// `<config dir>/extensions-allowed.json`: id → the fingerprint allowed.
const ALLOWED_FILE: &str = "extensions-allowed.json";
pub const MANIFEST_VERSION: u32 = 1;
pub const PAYLOAD_KIND: &str = "extensionAction";

const MAX_EXTENSIONS: usize = 50;
const MAX_ACTIONS: usize = 10;
const MAX_FIELDS: usize = 12;
const MAX_OPTIONS: usize = 100;
const MAX_ARGS: usize = 32;
const MAX_TEXT: usize = 200;
const MAX_DESCRIPTION: usize = 1_000;
const MAX_VALUE: usize = 20_000;
const MAX_ICON_BYTES: usize = 32 * 1024;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// What the fingerprint reads: a script folder, not a project checkout.
const MAX_FILES: usize = 300;
const MAX_FOLDER_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_STDOUT: usize = 64 * 1024;
/// The clicked message's text, and all of a conversation's text together.
const MAX_BODY_CHARS: usize = 100_000;
const MAX_ANSWER_CHARS: usize = 300;
const MAX_CONFIRM_TITLE: usize = 200;
const MAX_CONFIRM_LINE: usize = 300;
const MAX_CONFIRM_STEPS: usize = 12;
const MAX_CONFIRM_CHOICES: usize = 20;
const MAX_CONFIRM_LABEL: usize = 40;
const MAX_CONFIRM_STATE_BYTES: usize = 8 * 1024;
/// Confirm answers per click; one more is an error.
pub const MAX_CONFIRM_ROUNDS: u32 = 3;
const MAX_URL: usize = 2_000;

// ---------- manifest ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    penguin_extension: u32,
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon: Option<String>,
    actions: Vec<ManifestAction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestAction {
    id: String,
    title: String,
    #[serde(default)]
    icon: Option<String>,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    include_body: bool,
    #[serde(default)]
    include_thread: bool,
    #[serde(default)]
    form: Vec<FormField>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    #[default]
    Text,
    Textarea,
    Select,
    Checkbox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

/// One input of an action's form. `default` may use `{{subject}}`,
/// `{{from.name}}`, `{{from.email}}`, `{{snippet}}` (filled in by the UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormField {
    pub id: String,
    pub label: String,
    #[serde(rename = "type", default)]
    pub kind: FieldKind,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<FieldOption>,
    #[serde(default)]
    pub required: bool,
}

// ---------- what the UI sees ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionState {
    /// Found but not allowed yet.
    Off,
    On,
    /// Allowed once, but a file in the folder changed since.
    Changed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The folder's name inside the extensions folder.
    pub folder: String,
    /// A built-in icon name, when the manifest names one.
    pub icon: Option<String>,
    /// The extension's own SVG icon as a data: URL (shown in an <img>).
    pub icon_svg: Option<String>,
    pub state: ExtensionState,
    pub actions: Vec<ActionInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionInfo {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub icon_svg: Option<String>,
    pub include_body: bool,
    /// Gets every message of the conversation, with their text.
    pub include_thread: bool,
    pub form: Vec<FormField>,
    /// The program and its arguments, for Settings to show what runs.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionProblem {
    pub folder: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionsOverview {
    /// The extensions folder (created on first look).
    pub dir: String,
    pub extensions: Vec<ExtensionInfo>,
    /// Folders that aren't valid extensions, and why.
    pub problems: Vec<ExtensionProblem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunExtensionRequest {
    pub extension_id: String,
    pub action_id: String,
    pub account_id: String,
    pub message_id: String,
    #[serde(default)]
    pub form: BTreeMap<String, String>,
    /// Set when the person confirmed what the previous run asked.
    #[serde(default)]
    pub continuation: Option<ContinuationIn>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationIn {
    /// The confirm's `state`, exactly as the program returned it.
    #[serde(default)]
    pub state: Option<String>,
    /// The picked choice's value, when the confirm had choices.
    #[serde(default)]
    pub choice: Option<String>,
    /// Confirms answered so far for this click: 1 for the first.
    pub round: u32,
}

/// What the program answered: a line to show and a link to offer, or a
/// confirm step to show before running again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionResult {
    pub message: Option<String>,
    pub url: Option<String>,
    pub confirm: Option<ExtensionConfirm>,
}

// ---------- asking before acting (the confirm answer) ----------

/// `{"confirm": {...}}` on stdout, and nothing else beside it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ConfirmAnswer {
    pub confirm: ExtensionConfirm,
}

/// What Penguin shows before running the action again. Limits are checked
/// strictly, like the manifest's: a confirm that breaks one is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExtensionConfirm {
    /// What will happen, e.g. "Close FOR-123 with a comment". At most 200 characters.
    pub title: String,
    /// One line of context under the title. At most 300 characters.
    #[serde(default)]
    pub detail: Option<String>,
    /// What confirming will do, in order. At most 12.
    #[serde(default)]
    pub steps: Vec<ConfirmStep>,
    /// When present (1–20), the person picks one before confirming.
    #[serde(default)]
    pub choices: Vec<ConfirmChoice>,
    /// The question above the choices, e.g. "Which task?".
    #[serde(default)]
    pub choice_label: Option<String>,
    /// The confirm button's text, e.g. "Close task". At most 40 characters.
    #[serde(default)]
    pub confirm_label: Option<String>,
    /// Opaque; comes back unchanged as `continuation.state`. At most 8 KB.
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ConfirmStep {
    /// At most 300 characters.
    pub text: String,
    /// An http(s) link for the step.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ConfirmChoice {
    /// Comes back as `continuation.choice`. Unique, at most 200 characters.
    pub value: String,
    pub label: String,
    /// One muted line under the label.
    #[serde(default)]
    pub detail: Option<String>,
}

// ---------- the stdin payload (extensionAction v1) ----------

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct NamedRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ExtensionActionOut {
    pub extension: NamedRef,
    pub action: NamedRef,
    pub account_id: String,
    /// The account's address.
    pub account_email: String,
    pub thread_id: String,
    /// False: the message's `text` is empty and `fullText` null (the action
    /// doesn't ask for message bodies).
    pub includes_body: bool,
    /// True: `thread` is there (the action asks for the whole conversation).
    pub includes_thread: bool,
    pub message: MessageOut,
    /// The whole conversation; only when the action has `includeThread`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<ExtensionThreadOut>,
    /// The conversation in the provider's web app (Gmail), when there is one.
    pub web_url: Option<String>,
    /// The form's answers by field id; a checkbox is "true" or "false".
    pub form: BTreeMap<String, String>,
    /// Only on a run after the person confirmed what the program asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ContinuationOut>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ContinuationOut {
    /// The confirm's `state`, unchanged (null when it had none).
    pub state: Option<String>,
    /// The picked choice's `value`; null when the confirm had no choices.
    pub choice: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ExtensionThreadOut {
    /// Oldest first; the clicked message is one of them.
    pub messages: Vec<ThreadMessageOut>,
    /// The conversation's text didn't fit in 100,000 characters: the oldest
    /// messages were left out (and the newest cut, if it alone was too long).
    pub truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct ThreadMessageOut {
    pub id: String,
    pub date_iso: String,
    pub from: AddressOut,
    pub to: Vec<AddressOut>,
    pub cc: Vec<AddressOut>,
    pub subject: String,
    /// Plain text the sender wrote, with quoted reply history removed.
    pub text: String,
    /// `text` was cut to fit.
    pub truncated: bool,
    /// The body couldn't be downloaded (offline, or the server no longer has
    /// it), so `text` is empty.
    pub body_pending: bool,
}

// ---------- loading ----------

struct Loaded {
    folder: PathBuf,
    folder_name: String,
    manifest: Manifest,
    fingerprint: String,
}

pub fn extensions_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(DIR_NAME)
}

fn valid_id(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 40
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A form field's id: the key the program reads, so camelCase is fine.
fn valid_field_id(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 40
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("{what} is empty"));
    }
    if s.chars().count() > max || s.chars().any(char::is_control) {
        return Err(format!("{what} is too long or has control characters"));
    }
    Ok(())
}

/// A relative path that stays inside the folder (no `..`, not absolute).
fn inside(folder: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let ok = !rel.is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    ok.then(|| folder.join(p))
}

/// The program to run: an absolute path, or one inside the extension folder.
fn program_path(folder: &Path, program: &str) -> Result<PathBuf, String> {
    let p = Path::new(program);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    inside(folder, program).ok_or_else(|| {
        format!("program \"{program}\" must be absolute or inside the extension's folder")
    })
}

fn validate(m: &Manifest, folder: &Path) -> Result<(), String> {
    if m.penguin_extension != MANIFEST_VERSION {
        return Err(format!(
            "penguinExtension is {}; this Penguin reads version {MANIFEST_VERSION}",
            m.penguin_extension
        ));
    }
    if !valid_id(&m.id) {
        return Err("id must be 1–40 lowercase letters, digits or dashes".into());
    }
    check_text("name", &m.name, MAX_TEXT)?;
    if m.description.chars().count() > MAX_DESCRIPTION {
        return Err("description is too long".into());
    }
    if m.actions.is_empty() || m.actions.len() > MAX_ACTIONS {
        return Err(format!("an extension has 1–{MAX_ACTIONS} actions"));
    }
    let mut action_ids = HashSet::new();
    for a in &m.actions {
        if !valid_id(&a.id) || !action_ids.insert(a.id.as_str()) {
            return Err(format!("action id \"{}\" is invalid or repeated", a.id));
        }
        check_text("an action's title", &a.title, MAX_TEXT)?;
        program_path(folder, &a.program)?;
        if a.args.len() > MAX_ARGS || a.args.iter().any(|x| x.len() > 1_000 || x.contains('\0')) {
            return Err(format!(
                "action \"{}\" has too many or too long arguments",
                a.id
            ));
        }
        if a.timeout_secs
            .is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS)
        {
            return Err(format!("timeoutSecs is 1–{MAX_TIMEOUT_SECS}"));
        }
        if a.form.len() > MAX_FIELDS {
            return Err(format!("a form has at most {MAX_FIELDS} fields"));
        }
        let mut field_ids = HashSet::new();
        for f in &a.form {
            if !valid_field_id(&f.id) || !field_ids.insert(f.id.as_str()) {
                return Err(format!("form field id \"{}\" is invalid or repeated", f.id));
            }
            check_text("a field's label", &f.label, MAX_TEXT)?;
            if f.default
                .as_ref()
                .is_some_and(|d| d.chars().count() > MAX_VALUE)
            {
                return Err(format!("field \"{}\": default is too long", f.id));
            }
            match f.kind {
                FieldKind::Select => {
                    if f.options.is_empty() || f.options.len() > MAX_OPTIONS {
                        return Err(format!("select \"{}\" needs 1–{MAX_OPTIONS} options", f.id));
                    }
                    for o in &f.options {
                        check_text("an option's label", &o.label, MAX_TEXT)?;
                        if o.value.chars().count() > MAX_TEXT {
                            return Err("an option's value is too long".into());
                        }
                    }
                }
                _ if !f.options.is_empty() => {
                    return Err(format!("only a select has options (field \"{}\")", f.id));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// SHA-256 over every file in the folder (paths and bytes, in path order),
/// skipping dot-folders such as `.git`. Too many or too large files fail.
fn fingerprint(folder: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("can't read the folder: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if ft.is_dir() {
                if !hidden {
                    stack.push(path);
                }
            } else if ft.is_file() || ft.is_symlink() {
                total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(path);
            }
            if files.len() > MAX_FILES || total > MAX_FOLDER_BYTES {
                return Err(format!(
                    "the folder is too big (at most {MAX_FILES} files, {} MB)",
                    MAX_FOLDER_BYTES >> 20
                ));
            }
        }
    }
    files.sort();
    let mut h = Sha256::new();
    for f in files {
        let rel = f
            .strip_prefix(folder)
            .unwrap_or(&f)
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(&f).unwrap_or_default();
        h.update((rel.len() as u64).to_le_bytes());
        h.update(rel.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn load_one(folder: &Path) -> Result<Loaded, String> {
    let path = folder.join(MANIFEST);
    let meta = std::fs::metadata(&path).map_err(|_| format!("no {MANIFEST}"))?;
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!("{MANIFEST} is too big"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("can't read {MANIFEST}: {e}"))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    validate(&manifest, folder)?;
    Ok(Loaded {
        folder: folder.to_path_buf(),
        folder_name: folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        fingerprint: fingerprint(folder)?,
        manifest,
    })
}

/// Every folder in the extensions folder: the valid ones (first by folder
/// name wins a repeated id) and why the others aren't.
fn scan(dir: &Path) -> (Vec<Loaded>, Vec<ExtensionProblem>) {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                // A symlink to a checkout counts (is_dir follows links).
                .filter(|e| e.path().is_dir())
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    folders.sort();
    let mut found: Vec<Loaded> = Vec::new();
    let mut problems = Vec::new();
    for folder in folders.into_iter().take(MAX_EXTENSIONS) {
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match load_one(&folder) {
            Ok(l) if found.iter().any(|f| f.manifest.id == l.manifest.id) => {
                problems.push(ExtensionProblem {
                    folder: name,
                    error: format!("another folder already uses the id \"{}\"", l.manifest.id),
                })
            }
            Ok(l) => found.push(l),
            Err(error) => problems.push(ExtensionProblem {
                folder: name,
                error,
            }),
        }
    }
    (found, problems)
}

/// An icon value: a `.svg` file in the folder (as a data: URL), else a
/// built-in icon name.
fn icon(folder: &Path, value: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return (None, None);
    };
    if !v.to_ascii_lowercase().ends_with(".svg") {
        let name = v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        return (name.then(|| v.to_string()), None);
    }
    let svg = inside(folder, v)
        .and_then(|p| std::fs::read(p).ok())
        .filter(|b| b.len() <= MAX_ICON_BYTES)
        .filter(|b| String::from_utf8_lossy(b).contains("<svg"))
        .map(|b| {
            format!(
                "data:image/svg+xml;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b)
            )
        });
    (None, svg)
}

fn info(l: &Loaded, allowed: &HashMap<String, String>) -> ExtensionInfo {
    let m = &l.manifest;
    let state = match allowed.get(&m.id) {
        Some(fp) if *fp == l.fingerprint => ExtensionState::On,
        Some(_) => ExtensionState::Changed,
        None => ExtensionState::Off,
    };
    let (ext_icon, ext_svg) = icon(&l.folder, m.icon.as_deref());
    ExtensionInfo {
        id: m.id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        folder: l.folder_name.clone(),
        icon: ext_icon.clone(),
        icon_svg: ext_svg.clone(),
        state,
        actions: m
            .actions
            .iter()
            .map(|a| {
                let (i, svg) = match a.icon.as_deref() {
                    Some(v) => icon(&l.folder, Some(v)),
                    None => (ext_icon.clone(), ext_svg.clone()),
                };
                ActionInfo {
                    id: a.id.clone(),
                    title: a.title.clone(),
                    icon: i,
                    icon_svg: svg,
                    include_body: a.include_body,
                    include_thread: a.include_thread,
                    form: a.form.clone(),
                    command: std::iter::once(a.program.clone())
                        .chain(a.args.iter().cloned())
                        .collect(),
                }
            })
            .collect(),
    }
}

// ---------- approvals ----------

#[derive(Debug, Default, Serialize, Deserialize)]
struct AllowedFile {
    #[serde(default)]
    allowed: HashMap<String, String>,
}

fn load_allowed(config_dir: &Path) -> HashMap<String, String> {
    std::fs::read(config_dir.join(ALLOWED_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<AllowedFile>(&b).ok())
        .map(|f| f.allowed)
        .unwrap_or_default()
}

fn save_allowed(config_dir: &Path, allowed: HashMap<String, String>) -> std::io::Result<()> {
    let path = config_dir.join(ALLOWED_FILE);
    crate::share::config::write_atomic(
        &path,
        &serde_json::to_vec_pretty(&AllowedFile { allowed })?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn overview(config_dir: &Path) -> ExtensionsOverview {
    let dir = extensions_dir(config_dir);
    let _ = std::fs::create_dir_all(&dir);
    let (found, problems) = scan(&dir);
    let allowed = load_allowed(config_dir);
    ExtensionsOverview {
        dir: dir.display().to_string(),
        extensions: found.iter().map(|l| info(l, &allowed)).collect(),
        problems,
    }
}

/// Allow an extension as its folder is now, or take the approval back.
pub fn set_allowed(
    config_dir: &Path,
    id: &str,
    allow: bool,
) -> Result<ExtensionsOverview, CmdError> {
    let mut allowed = load_allowed(config_dir);
    if allow {
        let (found, _) = scan(&extensions_dir(config_dir));
        let l = found
            .iter()
            .find(|l| l.manifest.id == id)
            .ok_or_else(|| CmdError::not_found(format!("no extension \"{id}\"")))?;
        allowed.insert(id.to_string(), l.fingerprint.clone());
    } else {
        allowed.remove(id);
    }
    save_allowed(config_dir, allowed)
        .map_err(|e| CmdError::other(format!("couldn't save: {e}")))?;
    tracing::info!(extension = id, allow, "extension approval changed");
    Ok(overview(config_dir))
}

// ---------- running ----------

/// The form's answers, checked against the action's fields: only declared
/// fields, a select's value one of its options, required ones filled.
fn check_form(
    fields: &[FormField],
    form: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if let Some(k) = form.keys().find(|k| !fields.iter().any(|f| &f.id == *k)) {
        return Err(format!("unknown form field \"{k}\""));
    }
    let mut out = BTreeMap::new();
    for f in fields {
        let v = form.get(&f.id).map(String::as_str).unwrap_or("");
        if v.chars().count() > MAX_VALUE {
            return Err(format!("{} is too long", f.label));
        }
        let v = match f.kind {
            FieldKind::Checkbox => (if v == "true" { "true" } else { "false" }).to_string(),
            FieldKind::Select if !v.is_empty() && !f.options.iter().any(|o| o.value == v) => {
                return Err(format!("{}: not one of the choices", f.label));
            }
            _ => v.to_string(),
        };
        if f.required && (v.trim().is_empty() || (f.kind == FieldKind::Checkbox && v != "true")) {
            return Err(format!("{} is required", f.label));
        }
        out.insert(f.id.clone(), v);
    }
    Ok(out)
}

/// Gmail's web link for a conversation, for addresses that need no escaping.
fn gmail_url(email: &str, thread_id: &str) -> Option<String> {
    let safe = |s: &str, extra: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
    };
    (safe(email, "@._+-") && safe(thread_id, ""))
        .then(|| format!("https://mail.google.com/mail/u/{email}/#all/{thread_id}"))
}

/// A conversation's messages (oldest first) with their text, at most
/// `MAX_BODY_CHARS` in all: the newest are kept, and from the first that
/// doesn't fit on, older ones are left out. `pending`: ids with no body.
fn thread_payload(messages: &[Message], pending: &[String]) -> ExtensionThreadOut {
    let opts = ThreadOptions {
        include_full_text: false,
        max_chars_per_message: None,
    };
    let mut budget = MAX_BODY_CHARS;
    let mut truncated = false;
    let mut kept = Vec::new();
    for m in messages.iter().rev() {
        let body_pending = pending.contains(&m.id);
        let out = output::message_out(m, opts, body_pending);
        let text = if body_pending {
            String::new()
        } else {
            out.text
        };
        let len = text.chars().count();
        let (text, cut) = if len <= budget {
            (text, false)
        } else if kept.is_empty() {
            // The newest message alone is too long: keep its start.
            output::cap(&text, Some(budget))
        } else {
            truncated = true;
            break;
        };
        budget -= text.chars().count();
        truncated |= cut;
        kept.push(ThreadMessageOut {
            id: out.id,
            date_iso: out.date_iso,
            from: out.from,
            to: out.to,
            cc: out.cc,
            subject: out.subject,
            text,
            truncated: cut,
            body_pending,
        });
        if cut {
            break;
        }
    }
    kept.reverse();
    ExtensionThreadOut {
        messages: kept,
        truncated,
    }
}

/// `thread` is used only when the action has `includeThread`; `pending`
/// lists the messages (of the thread, or the clicked one) with no body.
#[allow(clippy::too_many_arguments)]
fn payload(
    l: &Loaded,
    a: &ManifestAction,
    message: &Message,
    thread: Option<&ThreadDetail>,
    pending: &[String],
    account_email: &str,
    web_url: Option<String>,
    form: BTreeMap<String, String>,
    continuation: Option<ContinuationOut>,
) -> Envelope<ExtensionActionOut> {
    let opts = ThreadOptions {
        include_full_text: false,
        max_chars_per_message: Some(MAX_BODY_CHARS),
    };
    let mut out = output::message_out(message, opts, pending.contains(&message.id));
    if !a.include_body {
        out.text = String::new();
        out.quoted_chars = 0;
        out.full_text = None;
        out.truncated = false;
    }
    output::envelope(
        PAYLOAD_KIND,
        ExtensionActionOut {
            extension: NamedRef {
                id: l.manifest.id.clone(),
                name: l.manifest.name.clone(),
            },
            action: NamedRef {
                id: a.id.clone(),
                name: a.title.clone(),
            },
            account_id: message.account_id.clone(),
            account_email: account_email.to_string(),
            thread_id: message.thread_id.clone(),
            includes_body: a.include_body,
            includes_thread: a.include_thread,
            message: out,
            thread: thread
                .filter(|_| a.include_thread)
                .map(|t| thread_payload(&t.messages, pending)),
            web_url,
            form,
            continuation,
        },
    )
}

fn web_link(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= MAX_URL
        && !u.chars().any(char::is_whitespace)
}

/// An optional line: absent or blank is None; else checked like a title.
fn optional_line(what: &str, v: &Option<String>, max: usize) -> Result<Option<String>, String> {
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) => check_text(what, t, max).map(|()| Some(t.to_string())),
    }
}

/// The confirm's limits; returns it with blank optional lines as None.
fn check_confirm(c: ExtensionConfirm) -> Result<ExtensionConfirm, String> {
    check_text("title", &c.title, MAX_CONFIRM_TITLE)?;
    let detail = optional_line("detail", &c.detail, MAX_CONFIRM_LINE)?;
    if c.steps.len() > MAX_CONFIRM_STEPS {
        return Err(format!("at most {MAX_CONFIRM_STEPS} steps"));
    }
    for st in &c.steps {
        check_text("a step's text", &st.text, MAX_CONFIRM_LINE)?;
        if st.url.as_deref().is_some_and(|u| !web_link(u)) {
            return Err("a step's url must be an http or https link".into());
        }
    }
    if c.choices.len() > MAX_CONFIRM_CHOICES {
        return Err(format!("at most {MAX_CONFIRM_CHOICES} choices"));
    }
    let mut values = HashSet::new();
    let mut choices = Vec::with_capacity(c.choices.len());
    for ch in c.choices {
        check_text("a choice's value", &ch.value, MAX_TEXT)?;
        if !values.insert(ch.value.clone()) {
            return Err(format!("the choice value \"{}\" is repeated", ch.value));
        }
        check_text("a choice's label", &ch.label, MAX_TEXT)?;
        let detail = optional_line("a choice's detail", &ch.detail, MAX_CONFIRM_LINE)?;
        choices.push(ConfirmChoice { detail, ..ch });
    }
    let choice_label = optional_line("choiceLabel", &c.choice_label, MAX_TEXT)?;
    let confirm_label = optional_line("confirmLabel", &c.confirm_label, MAX_CONFIRM_LABEL)?;
    if c.state
        .as_ref()
        .is_some_and(|st| st.len() > MAX_CONFIRM_STATE_BYTES)
    {
        return Err(format!(
            "state is over {} KB",
            MAX_CONFIRM_STATE_BYTES / 1024
        ));
    }
    Ok(ExtensionConfirm {
        detail,
        choices,
        choice_label,
        confirm_label,
        ..c
    })
}

/// What the program printed: `{"confirm": {...}}` (checked strictly),
/// `{"message", "url"}` JSON, else its first line. A url that isn't
/// http(s) is dropped. Errs only for a confirm that isn't valid.
pub fn parse_answer(stdout: &[u8]) -> Result<ExtensionResult, String> {
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim();
    let short = |s: &str| -> Option<String> {
        let s: String = s
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_ANSWER_CHARS)
            .collect();
        (!s.is_empty()).then_some(s)
    };
    if text.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if v.get("confirm").is_some() {
                let answer: ConfirmAnswer = serde_json::from_value(v).map_err(|e| e.to_string())?;
                return Ok(ExtensionResult {
                    confirm: Some(check_confirm(answer.confirm)?),
                    ..Default::default()
                });
            }
            let url = v["url"]
                .as_str()
                .filter(|u| web_link(u))
                .map(str::to_string);
            return Ok(ExtensionResult {
                message: v["message"].as_str().and_then(short),
                url,
                confirm: None,
            });
        }
    }
    Ok(ExtensionResult {
        message: text.lines().find(|l| !l.trim().is_empty()).and_then(short),
        ..Default::default()
    })
}

/// A continuation from the UI: within the round limit, sizes in bounds.
fn check_continuation(c: &ContinuationIn) -> Result<ContinuationOut, String> {
    if c.round == 0 || c.round > MAX_CONFIRM_ROUNDS {
        return Err(format!(
            "a click confirms at most {MAX_CONFIRM_ROUNDS} times"
        ));
    }
    if c.state
        .as_ref()
        .is_some_and(|st| st.len() > MAX_CONFIRM_STATE_BYTES)
        || c.choice
            .as_ref()
            .is_some_and(|ch| ch.chars().count() > MAX_TEXT)
    {
        return Err("the confirmation is too long".into());
    }
    Ok(ContinuationOut {
        state: c.state.clone(),
        choice: c.choice.clone(),
    })
}

/// The program's answer to this run, with the confirm round limit applied:
/// `round` confirms were answered before it.
fn answer_for_round(stdout: &[u8], round: u32, name: &str) -> CmdResult<ExtensionResult> {
    let r = parse_answer(stdout).map_err(|e| {
        CmdError::other(format!(
            "{name} asked to confirm, but its answer isn't valid: {e}"
        ))
    })?;
    if r.confirm.is_some() && round >= MAX_CONFIRM_ROUNDS {
        return Err(CmdError::other(format!(
            "{name} asked to confirm more than {MAX_CONFIRM_ROUNDS} times, so Penguin stopped"
        )));
    }
    Ok(r)
}

async fn run(state: AppStateRef<'_>, req: RunExtensionRequest) -> CmdResult<ExtensionResult> {
    let config_dir = state.paths.config_dir.clone();
    let ext_id = req.extension_id.clone();
    let (l, allowed) = blocking(move || {
        let (found, _) = scan(&extensions_dir(&config_dir));
        let allowed = load_allowed(&config_dir);
        Ok((found.into_iter().find(|l| l.manifest.id == ext_id), allowed))
    })
    .await?;
    let l =
        l.ok_or_else(|| CmdError::not_found(format!("no extension \"{}\"", req.extension_id)))?;
    match allowed.get(&l.manifest.id) {
        Some(fp) if *fp == l.fingerprint => {}
        Some(_) => {
            return Err(CmdError::denied(format!(
                "{} changed since you allowed it. Allow it again in Settings → Extensions.",
                l.manifest.name
            )))
        }
        None => {
            return Err(CmdError::denied(format!(
                "{} isn't allowed yet. Turn it on in Settings → Extensions.",
                l.manifest.name
            )))
        }
    }
    let a = l
        .manifest
        .actions
        .iter()
        .find(|a| a.id == req.action_id)
        .ok_or_else(|| {
            CmdError::not_found(format!(
                "{} has no action \"{}\"",
                l.manifest.name, req.action_id
            ))
        })?;
    let form = check_form(&a.form, &req.form).map_err(CmdError::invalid)?;
    let continuation = req
        .continuation
        .as_ref()
        .map(check_continuation)
        .transpose()
        .map_err(CmdError::invalid)?;
    let round = req.continuation.as_ref().map_or(0, |c| c.round);

    let account = state.account(&req.account_id).await?;
    let read = || {
        let store = state.store.clone();
        let (acct, mid) = (req.account_id.clone(), req.message_id.clone());
        let with_thread = a.include_thread;
        blocking(move || {
            let message = store
                .get_message(&acct, &mid)?
                .ok_or_else(|| CmdError::not_found("That message isn't on this Mac"))?;
            let (thread, pending) = if with_thread {
                (
                    store.get_thread(&acct, &message.thread_id)?,
                    store.pending_message_ids(&acct, &message.thread_id)?,
                )
            } else {
                let pending = store.is_body_pending(&acct, &mid)? == Some(true);
                (None, if pending { vec![mid] } else { Vec::new() })
            };
            Ok((message, thread, pending))
        })
    };
    let (mut message, mut thread, mut pending) = read().await?;
    // Text that goes along but isn't downloaded yet (headers-only, outside
    // the sync window): fetch it now, as quoting the original does. If that
    // fails (offline), the program still runs and sees `bodyPending`.
    if (a.include_body || a.include_thread) && !pending.is_empty() {
        let fetched = match state.provider(&req.account_id).await {
            Ok(provider) => provider
                .fetch_pending_bodies(&pending)
                .await
                .map_err(CmdError::from),
            Err(e) => Err(e),
        };
        match fetched {
            Ok(threads) => {
                if !threads.is_empty() {
                    state.emit_mail_changed(&req.account_id, threads);
                }
                (message, thread, pending) = read().await?;
            }
            Err(e) => tracing::warn!(
                extension = %l.manifest.id,
                action = %a.id,
                messages = pending.len(),
                code = ?e.code,
                error = %e.message,
                "could not download message bodies for an extension"
            ),
        }
    }
    let web_url = (account.provider == AccountProvider::Gmail)
        .then(|| gmail_url(&account.email, &message.thread_id))
        .flatten();
    let body = serde_json::to_vec(&payload(
        &l,
        a,
        &message,
        thread.as_ref(),
        &pending,
        &account.email,
        web_url,
        form,
        continuation,
    ))
    .map_err(|e| CmdError::other(e.to_string()))?;

    let program = program_path(&l.folder, &a.program).map_err(CmdError::invalid)?;
    let env = hooks::hook_env(&[
        ("PENGUIN_EXTENSION_ID", l.manifest.id.clone()),
        ("PENGUIN_ACTION_ID", a.id.clone()),
        ("PENGUIN_ACCOUNT", account.email.clone()),
    ]);
    let timeout = Duration::from_secs(a.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let outcome = hooks::run_program(
        &program,
        &a.args,
        &env,
        &body,
        timeout,
        &l.folder,
        Some(MAX_STDOUT),
    )
    .await;
    tracing::info!(
        extension = %l.manifest.id,
        action = %a.id,
        ok = outcome.is_ok(),
        "extension action ran"
    );
    match outcome {
        Ok(stdout) => answer_for_round(&stdout, round, &l.manifest.name),
        Err(e) => Err(CmdError::other(format!("{}: {e}", l.manifest.name))),
    }
}

// ---------- commands ----------

#[tauri::command]
pub async fn list_extensions(state: AppStateRef<'_>) -> CmdResult<ExtensionsOverview> {
    let dir = state.paths.config_dir.clone();
    blocking(move || Ok(overview(&dir))).await
}

#[tauri::command]
pub async fn set_extension_allowed(
    state: AppStateRef<'_>,
    id: String,
    allowed: bool,
) -> CmdResult<ExtensionsOverview> {
    let dir = state.paths.config_dir.clone();
    blocking(move || set_allowed(&dir, &id, allowed)).await
}

#[tauri::command]
pub async fn run_extension_action(
    state: AppStateRef<'_>,
    request: RunExtensionRequest,
) -> CmdResult<ExtensionResult> {
    run(state, request).await
}

#[tauri::command]
pub async fn open_extensions_folder(
    app: tauri::AppHandle,
    state: AppStateRef<'_>,
) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = extensions_dir(&state.paths.config_dir);
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::other(e.to_string()))?;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| CmdError::other(e.to_string()))
}

#[cfg(test)]
#[path = "extensions_tests.rs"]
mod tests;
