//! Usage: M0 chat service — owns one `claude` child process per session and
//! bridges its stream-json output to per-session Tauri events.
//!
//! Path B model (one session == one `claude` process):
//! - `create_session` spawns a dedicated [`ClaudeProc`] running the local
//!   `claude` CLI in `--input-format stream-json` mode; the process stays
//!   resident to handle multi-turn input.
//! - Sessions are tracked in-memory keyed by UUID; M0 keeps everything in
//!   RAM (no DB), wiped on app exit.
//! - Each process's callbacks are wired to `chat-event-{id}` /
//!   `chat-error-{id}` / `chat-exit-{id}` Tauri events.
//! - Validates session cwd (absolute + existing + not the AIO data dir).
//! - Per-session permission knobs (`permission_mode` / `allowed_tools` /
//!   `disallowed_tools`) flow through into [`ClaudeProcConfig`]. The
//!   coarse-grained allow/ask/deny rules stay in `claude_settings.rs` and the
//!   CLI manager page — `claude` reads those from `settings.json` itself, so
//!   chat does not duplicate that editor.
//!
//! Out of scope (later milestones):
//! - DB persistence and migration (M2).
//! - Process auto-restart and health checks (M3).
//! - Production-mode bundled CLI path resolution (M4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use rand::RngCore;
use serde::Serialize;
use tauri::Emitter;

use crate::infra::claude_proc::{ClaudeProc, ClaudeProcCallbacks, ClaudeProcConfig};
use crate::shared::error::{AppError, AppResult};

/// Tauri-managed state holding the chat service. Cloning is cheap.
#[derive(Default, Clone)]
pub(crate) struct ChatState {
    service: Arc<ChatService>,
}

impl ChatState {
    pub(crate) fn service(&self) -> Arc<ChatService> {
        self.service.clone()
    }
}

/// Core chat service. Lives behind an `Arc` inside `ChatState`.
///
/// Holds one resident `claude` process per live session. The map is the
/// sole owner of each [`ClaudeProc`]; removing an entry drops the handle,
/// and (because the process is spawned with `kill_on_drop`) tears the child
/// down. Callbacks only ever hold a [`Weak`] back-reference, so a process
/// cannot keep the service — or itself — alive.
#[derive(Default)]
pub(crate) struct ChatService {
    sessions: Mutex<HashMap<String, Arc<ClaudeProc>>>,
}

impl ChatService {
    /// Clone out the process handle for `session_id`, but only while it is
    /// still alive. A process that has exited (but whose `on_exit` cleanup
    /// has not yet removed the entry) is treated as gone, so callers see
    /// `CHAT_SESSION_NOT_FOUND` rather than dialing a dead pipe.
    fn proc(&self, session_id: &str) -> Option<Arc<ClaudeProc>> {
        let sessions = lock_sessions(&self.sessions).ok()?;
        sessions
            .get(session_id)
            .filter(|proc| proc.is_alive())
            .cloned()
    }

    fn insert_session(&self, session_id: String, proc: Arc<ClaudeProc>) -> AppResult<()> {
        let mut sessions = lock_sessions(&self.sessions)?;
        sessions.insert(session_id, proc);
        Ok(())
    }

    /// Drop the process handle for `session_id` and return it (if present)
    /// so the caller can `close()` it outside the lock. A poisoned lock is
    /// treated as "nothing to remove" rather than surfacing an error — the
    /// caller's contract (idempotent close) does not benefit from failing.
    fn take_session(&self, session_id: &str) -> Option<Arc<ClaudeProc>> {
        let mut sessions = lock_sessions(&self.sessions).ok()?;
        sessions.remove(session_id)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatErrorPayload {
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct ChatExitPayload {
    code: Option<i32>,
}

/// Create a chat session bound to `cwd`.
///
/// 1. Canonicalises `cwd` and rejects anything that is not an existing
///    directory or that lives inside the AIO data dir.
/// 2. Resolves the local `claude` executable and spawns a dedicated
///    process for the session, wiring its callbacks to per-session events.
/// 3. Stores the handle and returns the freshly generated session id.
///
/// On spawn failure the session is not registered, so the frontend can
/// retry without leaking a half-open entry.
// The per-session knobs (cwd + permission/tool/launcher/model overrides) are a
// flat list mirroring `ChatCreateSessionInput`; grouping them into a struct
// would only move the noise, so we follow the crate-wide convention of allowing
// the lint here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session<R: tauri::Runtime>(
    service: Arc<ChatService>,
    app: tauri::AppHandle<R>,
    cwd: String,
    permission_mode: Option<String>,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    launcher: Option<String>,
    model: Option<String>,
) -> AppResult<String> {
    let cwd = validate_cwd(&app, &cwd)?;
    let session_id = generate_uuid_v4();

    let preferred = normalize_launcher(launcher);
    let claude_path = resolve_chat_launcher_path(preferred).ok_or_else(|| {
        // Distinct from the process-down codes claude_proc returns: this is a
        // setup error (no launcher on PATH), so the frontend can prompt the
        // user to install/locate the CLI rather than retry the spawn. The
        // message names exactly what was requested (reclaude/claude/either).
        AppError::new(
            "CHAT_CLAUDE_NOT_FOUND",
            launcher_not_found_message(preferred),
        )
    })?;

    let config = ClaudeProcConfig {
        claude_path,
        cwd: cwd.to_string_lossy().into_owned(),
        session_id: session_id.clone(),
        permission_mode: normalize_permission_mode(permission_mode),
        model: normalize_model(model),
        allowed_tools: sanitize_tools(allowed_tools),
        disallowed_tools: sanitize_tools(disallowed_tools),
        // M0 grants no extra read roots beyond `cwd`; M1 will surface a picker.
        add_dirs: Vec::new(),
    };

    let callbacks = build_callbacks(Arc::downgrade(&service), &app, session_id.clone());
    let proc = ClaudeProc::spawn(config, callbacks).await?;

    service.insert_session(session_id.clone(), Arc::new(proc))?;
    Ok(session_id)
}

/// Forward a user message to the session's resident `claude` process.
pub(crate) async fn send_message(
    service: Arc<ChatService>,
    session_id: String,
    content: String,
) -> AppResult<()> {
    let proc = service.proc(&session_id).ok_or_else(|| {
        AppError::new(
            "CHAT_SESSION_NOT_FOUND",
            format!("session {session_id} not found"),
        )
    })?;
    proc.send_user_message(content).await
}

/// Tear down a session's process. Idempotent: an unknown session id (or one
/// whose process already exited) returns `Ok(())` so the frontend can retry
/// after a crash or reload.
pub(crate) async fn close_session(service: Arc<ChatService>, session_id: String) -> AppResult<()> {
    let Some(proc) = service.take_session(&session_id) else {
        return Ok(());
    };
    proc.close().await
}

/// Resolve a sensible default cwd for a new chat session.
///
/// Returns the user's home directory as an absolute path. The frontend
/// calls this through the `chat_default_cwd` IPC command — AIO does not
/// expose the Tauri `core:path:*` permissions to the webview by design
/// (see `capabilities/main-core.json`), so all path resolution happens
/// in Rust. M1 will replace this with a per-session cwd picker.
pub(crate) fn default_cwd<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<String> {
    let home = crate::infra::app_paths::home_dir(app)?;
    Ok(home.to_string_lossy().into_owned())
}

/// One slash-command candidate surfaced to the frontend's `/` autocomplete.
///
/// `source` is one of `"builtin"`, `"skill"`, or `"command"`. The command
/// layer maps this onto the Specta-annotated IPC type.
pub(crate) struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub source: &'static str,
}

/// Max bytes read from a `SKILL.md` / command `.md` when sniffing its
/// frontmatter. Mirrors the skills module's `SKILL_MD_MAX_BYTES`.
const SLASH_MD_MAX_BYTES: usize = 256 * 1024;
/// Cap on directory entries scanned per slash-command source, so a
/// pathological directory cannot stall the picker.
const SLASH_DIR_ENTRY_MAX: usize = 1024;

/// Best-effort, cwd-aware list of slash commands a headless `claude` is
/// likely to accept, for the frontend `/` autocomplete *before* a session
/// exists. Three sources are merged, de-duplicated by name (first wins):
///
/// 1. A hardcoded set of built-ins known to work headless.
/// 2. Installed skills under `~/.claude/skills/<name>/SKILL.md`.
/// 3. Custom commands under `~/.claude/commands/*.md` and
///    `<cwd>/.claude/commands/*.md`.
///
/// This is intentionally approximate — any source that is missing or
/// unreadable is silently skipped, and the frontend re-validates against the
/// authoritative `system`/`init` list once the session starts. Always
/// returns at least the built-ins.
pub(crate) fn list_slash_commands<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
) -> Vec<SlashCommand> {
    let mut out = builtin_slash_commands();

    if let Ok(home) = crate::infra::app_paths::home_dir(app) {
        let claude_dir = home.join(".claude");
        out.extend(scan_skill_dir(&claude_dir.join("skills")));
        out.extend(scan_command_dir(&claude_dir.join("commands")));
    }

    let cwd = cwd.trim();
    if !cwd.is_empty() {
        let project_commands = Path::new(cwd).join(".claude").join("commands");
        out.extend(scan_command_dir(&project_commands));
    }

    dedup_by_name(out)
}

/// Built-in slash commands that work in headless (`--print` / stream-json)
/// mode. Deliberately excludes `model` / `effort` / `permission-mode`: those
/// cannot be switched dynamically headless and the GUI exposes dedicated
/// selectors for them already.
fn builtin_slash_commands() -> Vec<SlashCommand> {
    [
        ("clear", "清空当前会话上下文，开始新对话"),
        ("compact", "压缩对话历史以释放上下文窗口"),
        ("context", "查看当前上下文占用情况"),
        ("usage", "查看本次会话的用量与额度"),
    ]
    .into_iter()
    .map(|(name, description)| SlashCommand {
        name: name.to_string(),
        description: Some(description.to_string()),
        source: "builtin",
    })
    .collect()
}

/// Scan immediate subdirectories of `skills_root`, listing each that contains
/// a `SKILL.md` — directory name as the command name, frontmatter
/// `description` (if any) as the description. Directories without a `SKILL.md`
/// are not skills and are skipped. Returns an empty vec if `skills_root` is
/// absent or unreadable.
fn scan_skill_dir(skills_root: &Path) -> Vec<SlashCommand> {
    let Ok(entries) = std::fs::read_dir(skills_root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten().take(SLASH_DIR_ENTRY_MAX) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        out.push(SlashCommand {
            name,
            description: read_frontmatter_description(&skill_md),
            source: "skill",
        });
    }
    out
}

/// Scan `commands_root` for top-level `*.md` files, taking the file stem as
/// the command name and the frontmatter `description`. Returns an empty vec
/// if the directory is absent or unreadable.
fn scan_command_dir(commands_root: &Path) -> Vec<SlashCommand> {
    let Ok(entries) = std::fs::read_dir(commands_root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten().take(SLASH_DIR_ENTRY_MAX) {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        let description = read_frontmatter_description(&path);
        out.push(SlashCommand {
            name: stem,
            description,
            source: "command",
        });
    }
    out
}

/// Read a markdown file (size-capped) and extract its YAML frontmatter
/// `description`, if any. Any read / decode / parse failure yields `None`
/// (the command stays listed, just without a description).
fn read_frontmatter_description(path: &Path) -> Option<String> {
    let bytes = crate::shared::fs::read_optional_file_with_max_len(path, SLASH_MD_MAX_BYTES)
        .ok()
        .flatten()?;
    let text = String::from_utf8(bytes).ok()?;
    frontmatter_description(&text)
}

/// Extract `description` from a leading `---`-delimited YAML frontmatter
/// block. Returns `None` when there is no frontmatter or no (non-blank)
/// `description` key. Only the simple `key: value` form is supported (enough
/// for SKILL.md / command frontmatter); quotes are stripped.
fn frontmatter_description(text: &str) -> Option<String> {
    let text = text.trim_start();
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "description" {
            continue;
        }
        let value = strip_matching_quotes(value.trim()).trim();
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

/// Strip a single pair of matching surrounding quotes (`"` or `'`).
fn strip_matching_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// De-duplicate by command name, keeping the first occurrence (so built-ins
/// win over skills win over custom commands, and user commands win over
/// project commands). Preserves insertion order.
fn dedup_by_name(commands: Vec<SlashCommand>) -> Vec<SlashCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(commands.len());
    for command in commands {
        if seen.insert(command.name.clone()) {
            out.push(command);
        }
    }
    out
}

/// Resolve the launcher used to start a chat session's `claude` process.
///
/// `preferred` lets the frontend force a specific launcher (see
/// [`normalize_launcher`] for the accepted values):
/// - `Some("reclaude")` — scan PATH for `reclaude` **only**; `None` if absent.
/// - `Some("claude")` — scan PATH for `claude` **only**; `None` if absent.
/// - `None` (auto) — the default order:
///   1. `AIO_CHAT_CLAUDE_LAUNCHER` env var (absolute path) — explicit override
///      for CI / non-standard installs.
///   2. `reclaude` on PATH — the user's normal launcher. It performs a config
///      sync (auth / endpoint setup, printed as a `同步配置…` line on **stderr**,
///      which `claude_proc` drains to the log rather than surfacing as an error)
///      then delegates to `claude`, forwarding every argument verbatim. Spawning
///      it makes chat use the exact launch path the user uses interactively, so
///      authentication and config match — a bare `claude` skips that sync and
///      can fail with 401.
///   3. `claude` on PATH — fallback when `reclaude` is not installed.
///
/// The `AIO_CHAT_CLAUDE_LAUNCHER` override only applies to auto: an explicit
/// `reclaude`/`claude` request is honoured verbatim so the user's choice wins.
/// PATH-only scanning is enough for production users (these launchers add
/// themselves to PATH); returns `None` when nothing is found so the caller can
/// surface a clear setup error.
fn resolve_chat_launcher_path(preferred: Option<&str>) -> Option<String> {
    match preferred {
        Some("reclaude") => find_launcher_on_path(&["reclaude"]),
        Some("claude") => find_launcher_on_path(&["claude"]),
        // Auto: env override first, then prefer `reclaude` over `claude`.
        _ => {
            if let Some(override_path) = std::env::var_os("AIO_CHAT_CLAUDE_LAUNCHER") {
                let candidate = PathBuf::from(override_path);
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
            find_launcher_on_path(&["reclaude", "claude"])
        }
    }
}

/// Scan PATH for the first existing launcher among `bases`, trying each
/// platform executable extension. `bases` is searched in order, so the caller
/// controls preference (e.g. `["reclaude", "claude"]` prefers `reclaude`).
fn find_launcher_on_path(bases: &[&str]) -> Option<String> {
    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ".ps1", ""]
    } else {
        &[""]
    };
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    for base in bases {
        for dir in &dirs {
            for ext in exts {
                let candidate = dir.join(format!("{base}{ext}"));
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

/// Normalise the frontend `launcher` field to a known launcher base name.
///
/// Accepts `"reclaude"` / `"claude"` (case-sensitive, trimmed); blank or any
/// unrecognised value maps to `None` (auto), since the IPC contract only ever
/// sends those two values and falling back to auto is safer than failing on a
/// typo or running an arbitrary binary name.
fn normalize_launcher(launcher: Option<String>) -> Option<&'static str> {
    match launcher.as_deref().map(str::trim) {
        Some("reclaude") => Some("reclaude"),
        Some("claude") => Some("claude"),
        _ => None,
    }
}

/// Human-readable "not found" message naming exactly what was searched for, so
/// the frontend can tell the user which launcher to install/locate.
fn launcher_not_found_message(preferred: Option<&str>) -> String {
    match preferred {
        Some(name) => format!("could not locate `{name}` on PATH"),
        None => "could not locate `reclaude` or `claude` on PATH".to_string(),
    }
}

/// Trim a `permission_mode` string and treat blank/empty as "unset" so the
/// process module can simply omit the `--permission-mode` flag.
fn normalize_permission_mode(mode: Option<String>) -> Option<String> {
    mode.map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
}

/// Trim a `model` string and treat blank/empty as "unset" so the process
/// module can simply omit the `--model` flag and let `claude` pick its
/// default. Any non-blank value is forwarded verbatim (the model alias /
/// id is validated upstream by `claude`, not here).
fn normalize_model(model: Option<String>) -> Option<String> {
    model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// Drop blank tool names that would otherwise become empty `--allowedTools`
/// / `--disallowedTools` entries.
fn sanitize_tools(tools: Vec<String>) -> Vec<String> {
    tools
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Build the callback set for a single session, forwarding the process's
/// stream-json output to per-session Tauri events.
///
/// Callbacks hold a [`Weak`] back-reference to the service so they never
/// keep it (or, transitively, the process) alive. `on_exit` removes the
/// session from the map — after which `send_message` reports
/// `CHAT_SESSION_NOT_FOUND` and `close_session` is a no-op — and emits a
/// `chat-exit-{id}` event so the frontend learns the session ended.
fn build_callbacks<R: tauri::Runtime>(
    service: Weak<ChatService>,
    app: &tauri::AppHandle<R>,
    session_id: String,
) -> ClaudeProcCallbacks {
    let app_for_event = app.clone();
    let event_name = format!("chat-event-{session_id}");
    let on_event = move |event: serde_json::Value| {
        if let Err(err) = app_for_event.emit(&event_name, event) {
            tracing::warn!(
                event = %event_name,
                error = %err,
                "failed to emit chat event",
            );
        }
    };

    let app_for_error = app.clone();
    let error_event = format!("chat-error-{session_id}");
    let on_error = move |error: String| {
        let payload = ChatErrorPayload { error };
        if let Err(err) = app_for_error.emit(&error_event, payload) {
            tracing::warn!(
                event = %error_event,
                error = %err,
                "failed to emit chat error",
            );
        }
    };

    let app_for_exit = app.clone();
    let exit_event = format!("chat-exit-{session_id}");
    let exit_session_id = session_id;
    let on_exit = move |code: Option<i32>| {
        if let Some(service) = service.upgrade() {
            // Idempotent: a session closed explicitly is already gone here.
            let _ = service.take_session(&exit_session_id);
        }
        let payload = ChatExitPayload { code };
        if let Err(err) = app_for_exit.emit(&exit_event, payload) {
            tracing::warn!(
                event = %exit_event,
                error = %err,
                "failed to emit chat exit",
            );
        }
    };

    ClaudeProcCallbacks {
        on_event: Box::new(on_event),
        on_error: Box::new(on_error),
        on_exit: Box::new(on_exit),
    }
}

fn lock_sessions(
    sessions: &Mutex<HashMap<String, Arc<ClaudeProc>>>,
) -> AppResult<std::sync::MutexGuard<'_, HashMap<String, Arc<ClaudeProc>>>> {
    sessions
        .lock()
        .map_err(|_| AppError::new("INTERNAL_ERROR", "chat session map poisoned".to_string()))
}

fn validate_cwd<R: tauri::Runtime>(app: &tauri::AppHandle<R>, raw: &str) -> AppResult<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(
            "CHAT_INVALID_CWD",
            "cwd must not be empty".to_string(),
        ));
    }

    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err(AppError::new(
            "CHAT_INVALID_CWD",
            format!("cwd must be absolute, got `{trimmed}`"),
        ));
    }

    let canonical = candidate.canonicalize().map_err(|e| {
        AppError::new(
            "CHAT_INVALID_CWD",
            format!("cwd `{trimmed}` could not be canonicalised: {e}"),
        )
    })?;

    if !canonical.is_dir() {
        return Err(AppError::new(
            "CHAT_INVALID_CWD",
            format!("cwd `{}` is not a directory", canonical.display()),
        ));
    }

    if let Ok(data_dir) = crate::infra::app_paths::app_data_dir(app) {
        if let Ok(canonical_data_dir) = data_dir.canonicalize() {
            if path_is_inside(&canonical, &canonical_data_dir) {
                return Err(AppError::new(
                    "CHAT_INVALID_CWD",
                    format!(
                        "cwd `{}` is inside AIO data dir and is not allowed",
                        canonical.display()
                    ),
                ));
            }
        }
    }

    Ok(canonical)
}

fn path_is_inside(candidate: &Path, ancestor: &Path) -> bool {
    let mut current = Some(candidate);
    while let Some(path) = current {
        if path == ancestor {
            return true;
        }
        current = path.parent();
    }
    false
}

/// Minimal RFC 4122 v4 UUID generation using `rand` (no new crate). Mirrors
/// how [`gateway::oauth::pkce`] sources randomness for similar token use.
fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn generate_uuid_v4_produces_correct_format() {
        let uuid = generate_uuid_v4();
        // 36 chars: 8-4-4-4-12 hex with dashes.
        assert_eq!(uuid.len(), 36);
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);

        // Version nibble (1st char of group 3) must be '4'.
        let version_char = parts[2].chars().next().expect("version nibble");
        assert_eq!(version_char, '4');

        // Variant nibble (1st char of group 4) must be 8/9/a/b.
        let variant_char = parts[3].chars().next().expect("variant nibble");
        assert!(
            matches!(variant_char, '8' | '9' | 'a' | 'b'),
            "variant nibble was {variant_char}",
        );
    }

    #[test]
    fn generate_uuid_v4_is_random_across_calls() {
        let a = generate_uuid_v4();
        let b = generate_uuid_v4();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_permission_mode_treats_blank_as_unset() {
        assert_eq!(normalize_permission_mode(None), None);
        assert_eq!(normalize_permission_mode(Some(String::new())), None);
        assert_eq!(normalize_permission_mode(Some("   ".to_string())), None);
        assert_eq!(
            normalize_permission_mode(Some("  acceptEdits  ".to_string())),
            Some("acceptEdits".to_string())
        );
    }

    #[test]
    fn normalize_model_treats_blank_as_unset_and_trims() {
        assert_eq!(normalize_model(None), None);
        assert_eq!(normalize_model(Some(String::new())), None);
        assert_eq!(normalize_model(Some("   ".to_string())), None);
        assert_eq!(
            normalize_model(Some("  claude-sonnet-4-6  ".to_string())),
            Some("claude-sonnet-4-6".to_string())
        );
        // A bare alias is forwarded verbatim (validated upstream by claude).
        assert_eq!(
            normalize_model(Some("opus".to_string())),
            Some("opus".to_string())
        );
    }

    #[test]
    fn builtin_slash_commands_is_non_empty_and_tagged_builtin() {
        let builtins = builtin_slash_commands();
        assert!(!builtins.is_empty(), "builtin list must not be empty");
        assert!(builtins.iter().all(|c| c.source == "builtin"));
        assert!(builtins.iter().all(|c| c.description.is_some()));
        // The headless-safe set must NOT advertise dynamic-switch commands.
        let names: Vec<&str> = builtins.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"clear"));
        assert!(!names.contains(&"model"));
        assert!(!names.contains(&"permission-mode"));
    }

    #[test]
    fn frontmatter_description_parses_strips_quotes_and_handles_edge_cases() {
        // Happy path with double quotes.
        assert_eq!(
            frontmatter_description("---\nname: x\ndescription: \"do a thing\"\n---\nbody"),
            Some("do a thing".to_string())
        );
        // Single quotes + extra keys + comment-ish lines.
        assert_eq!(
            frontmatter_description("---\ndescription: 'hi there'\nother: 1\n---\n"),
            Some("hi there".to_string())
        );
        // No frontmatter at all.
        assert_eq!(frontmatter_description("# just markdown\n"), None);
        // Frontmatter present but no description key.
        assert_eq!(frontmatter_description("---\nname: only\n---\n"), None);
        // Blank description is treated as absent.
        assert_eq!(frontmatter_description("---\ndescription:   \n---\n"), None);
    }

    #[test]
    fn scan_command_dir_missing_dir_returns_empty_without_panic() {
        let missing = Path::new("/nope-no-such-commands-dir-xyz-12345");
        assert!(scan_command_dir(missing).is_empty());
    }

    #[test]
    fn scan_skill_dir_missing_dir_returns_empty_without_panic() {
        let missing = Path::new("/nope-no-such-skills-dir-xyz-12345");
        assert!(scan_skill_dir(missing).is_empty());
    }

    #[test]
    fn scan_command_dir_reads_md_files_with_and_without_frontmatter() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("review.md"),
            "---\ndescription: Review code\n---\nPrompt body",
        )
        .expect("write review.md");
        std::fs::write(dir.path().join("plain.md"), "no frontmatter here").expect("write plain.md");
        // Non-markdown files must be ignored.
        std::fs::write(dir.path().join("notes.txt"), "ignore me").expect("write notes.txt");

        let mut cmds = scan_command_dir(dir.path());
        cmds.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(cmds.len(), 2, "only the two .md files");
        assert_eq!(cmds[0].name, "plain");
        assert_eq!(cmds[0].description, None);
        assert_eq!(cmds[0].source, "command");
        assert_eq!(cmds[1].name, "review");
        assert_eq!(cmds[1].description, Some("Review code".to_string()));
    }

    #[test]
    fn scan_skill_dir_requires_skill_md_and_uses_dir_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let skill = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill).expect("create skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: ignored-name\ndescription: A handy skill\n---\n",
        )
        .expect("write SKILL.md");
        // A directory WITHOUT SKILL.md is not a skill and must be skipped.
        std::fs::create_dir_all(dir.path().join("bare-skill")).expect("create bare skill dir");

        let skills = scan_skill_dir(dir.path());

        assert_eq!(skills.len(), 1, "only the dir with SKILL.md");
        assert_eq!(skills[0].name, "my-skill");
        // Name comes from the directory, not the frontmatter `name`.
        assert_eq!(skills[0].description, Some("A handy skill".to_string()));
        assert_eq!(skills[0].source, "skill");
    }

    #[test]
    fn dedup_by_name_keeps_first_occurrence() {
        let input = vec![
            SlashCommand {
                name: "dup".to_string(),
                description: Some("first".to_string()),
                source: "builtin",
            },
            SlashCommand {
                name: "dup".to_string(),
                description: Some("second".to_string()),
                source: "command",
            },
            SlashCommand {
                name: "unique".to_string(),
                description: None,
                source: "skill",
            },
        ];
        let out = dedup_by_name(input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "dup");
        assert_eq!(out[0].source, "builtin");
        assert_eq!(out[0].description, Some("first".to_string()));
        assert_eq!(out[1].name, "unique");
    }

    #[test]
    fn sanitize_tools_drops_blank_entries_and_trims() {
        let input = vec![
            "  Read ".to_string(),
            String::new(),
            "Bash".to_string(),
            "   ".to_string(),
        ];
        assert_eq!(
            sanitize_tools(input),
            vec!["Read".to_string(), "Bash".to_string()]
        );
    }

    #[test]
    fn normalize_launcher_maps_known_values_and_defaults_to_auto() {
        assert_eq!(normalize_launcher(None), None);
        assert_eq!(normalize_launcher(Some(String::new())), None);
        assert_eq!(normalize_launcher(Some("   ".to_string())), None);
        // Unrecognised values fall back to auto rather than failing.
        assert_eq!(normalize_launcher(Some("codex".to_string())), None);
        assert_eq!(normalize_launcher(Some("Claude".to_string())), None);
        assert_eq!(
            normalize_launcher(Some("reclaude".to_string())),
            Some("reclaude")
        );
        assert_eq!(
            normalize_launcher(Some("  claude  ".to_string())),
            Some("claude")
        );
    }

    #[test]
    fn launcher_not_found_message_names_the_requested_launcher() {
        assert_eq!(
            launcher_not_found_message(Some("reclaude")),
            "could not locate `reclaude` on PATH"
        );
        assert_eq!(
            launcher_not_found_message(Some("claude")),
            "could not locate `claude` on PATH"
        );
        assert_eq!(
            launcher_not_found_message(None),
            "could not locate `reclaude` or `claude` on PATH"
        );
    }

    #[test]
    fn path_is_inside_detects_descendants() {
        let parent = Path::new("/foo/bar");
        assert!(path_is_inside(Path::new("/foo/bar"), parent));
        assert!(path_is_inside(Path::new("/foo/bar/baz"), parent));
        assert!(path_is_inside(Path::new("/foo/bar/baz/qux"), parent));
    }

    #[test]
    fn path_is_inside_rejects_siblings() {
        let parent = Path::new("/foo/bar");
        assert!(!path_is_inside(Path::new("/foo/baz"), parent));
        assert!(!path_is_inside(Path::new("/foo"), parent));
        assert!(!path_is_inside(Path::new("/other"), parent));
    }

    #[test]
    fn validate_cwd_rejects_relative_path() {
        let app = tauri::test::mock_app();
        let err = validate_cwd(&app.handle().clone(), "./relative").expect_err("must reject");
        assert!(
            err.to_string().starts_with("CHAT_INVALID_CWD:"),
            "got {err}",
        );
    }

    #[test]
    fn validate_cwd_rejects_empty_path() {
        let app = tauri::test::mock_app();
        let err = validate_cwd(&app.handle().clone(), "   ").expect_err("must reject");
        assert!(
            err.to_string().starts_with("CHAT_INVALID_CWD:"),
            "got {err}",
        );
    }

    #[test]
    fn validate_cwd_rejects_nonexistent_path() {
        let app = tauri::test::mock_app();
        // Absolute path that almost certainly does not exist.
        let bogus = if cfg!(windows) {
            "C:\\nope-does-not-exist-xyz-12345"
        } else {
            "/nope-does-not-exist-xyz-12345"
        };
        let err = validate_cwd(&app.handle().clone(), bogus).expect_err("must reject");
        assert!(
            err.to_string().starts_with("CHAT_INVALID_CWD:"),
            "got {err}",
        );
    }

    #[test]
    fn validate_cwd_accepts_existing_temp_dir() {
        let app = tauri::test::mock_app();
        let temp = tempfile::tempdir().expect("create tempdir");
        let path_str = temp
            .path()
            .to_str()
            .expect("tempdir path is utf8")
            .to_string();
        let canonical = validate_cwd(&app.handle().clone(), &path_str).expect("must accept");
        assert!(canonical.is_dir());
    }

    #[tokio::test]
    async fn send_message_returns_session_not_found_for_unknown_id() {
        let service = Arc::new(ChatService::default());
        let err = send_message(service, "missing-session".to_string(), "hello".to_string())
            .await
            .expect_err("must reject");
        assert!(
            err.to_string().starts_with("CHAT_SESSION_NOT_FOUND:"),
            "got {err}",
        );
    }

    #[tokio::test]
    async fn close_session_is_a_noop_for_unknown_id() {
        let service = Arc::new(ChatService::default());
        let result = close_session(service, "missing-session".to_string()).await;
        assert!(result.is_ok());
    }

    // ---- launcher resolution -------------------------------------------
    //
    // These mutate the process-wide `PATH` / `AIO_CHAT_CLAUDE_LAUNCHER` env
    // vars, so they serialise on a shared guard and snapshot/restore both
    // vars. All env-dependent values are captured *before* asserting, so a
    // failed assertion never leaves the environment corrupted for other
    // tests. No other test in the crate touches these vars.
    use std::sync::Mutex as StdMutex;
    static PATH_GUARD: StdMutex<()> = StdMutex::new(());

    /// Primary on-disk filename the scanner looks for; `is_file` only needs
    /// the file to exist, so one variant per platform is enough.
    fn launcher_filename(base: &str) -> String {
        if cfg!(windows) {
            format!("{base}.cmd")
        } else {
            base.to_string()
        }
    }

    fn write_fake_launcher(dir: &Path, base: &str) {
        std::fs::write(dir.join(launcher_filename(base)), b"#!/bin/sh\n")
            .expect("write fake launcher");
    }

    #[test]
    fn resolve_chat_launcher_path_honours_explicit_preference() {
        let _guard = PATH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev_path = env::var_os("PATH");
        let prev_override = env::var_os("AIO_CHAT_CLAUDE_LAUNCHER");

        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_launcher(dir.path(), "reclaude");
        write_fake_launcher(dir.path(), "claude");
        // An env override must NOT leak into explicit requests.
        env::set_var("AIO_CHAT_CLAUDE_LAUNCHER", "C:/nonexistent/override");
        env::set_var("PATH", dir.path());

        let reclaude = resolve_chat_launcher_path(Some("reclaude"));
        let claude = resolve_chat_launcher_path(Some("claude"));

        match prev_path {
            Some(p) => env::set_var("PATH", p),
            None => env::remove_var("PATH"),
        }
        match prev_override {
            Some(p) => env::set_var("AIO_CHAT_CLAUDE_LAUNCHER", p),
            None => env::remove_var("AIO_CHAT_CLAUDE_LAUNCHER"),
        }

        assert!(
            reclaude
                .as_deref()
                .is_some_and(|p| p.ends_with(&launcher_filename("reclaude"))),
            "expected reclaude path, got {reclaude:?}",
        );
        assert!(
            claude
                .as_deref()
                .is_some_and(|p| p.ends_with(&launcher_filename("claude"))),
            "expected claude path, got {claude:?}",
        );
    }

    #[test]
    fn resolve_chat_launcher_path_auto_prefers_reclaude() {
        let _guard = PATH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev_path = env::var_os("PATH");
        let prev_override = env::var_os("AIO_CHAT_CLAUDE_LAUNCHER");

        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_launcher(dir.path(), "reclaude");
        write_fake_launcher(dir.path(), "claude");
        // Auto path with no override: reclaude wins over claude.
        env::remove_var("AIO_CHAT_CLAUDE_LAUNCHER");
        env::set_var("PATH", dir.path());

        let auto = resolve_chat_launcher_path(None);

        match prev_path {
            Some(p) => env::set_var("PATH", p),
            None => env::remove_var("PATH"),
        }
        if let Some(p) = prev_override {
            env::set_var("AIO_CHAT_CLAUDE_LAUNCHER", p);
        }

        assert!(
            auto.as_deref()
                .is_some_and(|p| p.ends_with(&launcher_filename("reclaude"))),
            "auto should prefer reclaude, got {auto:?}",
        );
    }

    #[test]
    fn resolve_chat_launcher_path_explicit_returns_none_when_absent() {
        let _guard = PATH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev_path = env::var_os("PATH");
        let prev_override = env::var_os("AIO_CHAT_CLAUDE_LAUNCHER");

        let dir = tempfile::tempdir().expect("tempdir");
        // Only `claude` is present.
        write_fake_launcher(dir.path(), "claude");
        env::remove_var("AIO_CHAT_CLAUDE_LAUNCHER");
        env::set_var("PATH", dir.path());

        // Requesting reclaude must not silently fall back to claude.
        let reclaude = resolve_chat_launcher_path(Some("reclaude"));
        let claude = resolve_chat_launcher_path(Some("claude"));

        match prev_path {
            Some(p) => env::set_var("PATH", p),
            None => env::remove_var("PATH"),
        }
        if let Some(p) = prev_override {
            env::set_var("AIO_CHAT_CLAUDE_LAUNCHER", p);
        }

        assert!(
            reclaude.is_none(),
            "reclaude should be absent, got {reclaude:?}"
        );
        assert!(claude.is_some(), "claude should be found, got {claude:?}");
    }
}
