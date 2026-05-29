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
pub(crate) async fn create_session<R: tauri::Runtime>(
    service: Arc<ChatService>,
    app: tauri::AppHandle<R>,
    cwd: String,
    permission_mode: Option<String>,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
) -> AppResult<String> {
    let cwd = validate_cwd(&app, &cwd)?;
    let session_id = generate_uuid_v4();

    let claude_path = resolve_chat_launcher_path().ok_or_else(|| {
        // Distinct from the process-down codes claude_proc returns: this is a
        // setup error (no launcher on PATH), so the frontend can prompt the
        // user to install/locate the CLI rather than retry the spawn.
        AppError::new(
            "CHAT_CLAUDE_NOT_FOUND",
            "could not locate `reclaude` or `claude` on PATH".to_string(),
        )
    })?;

    let config = ClaudeProcConfig {
        claude_path,
        cwd: cwd.to_string_lossy().into_owned(),
        session_id: session_id.clone(),
        permission_mode: normalize_permission_mode(permission_mode),
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

/// Resolve the launcher used to start a chat session's `claude` process.
///
/// Preference order:
/// 1. `AIO_CHAT_CLAUDE_LAUNCHER` env var (absolute path) — explicit override
///    for CI / non-standard installs.
/// 2. `reclaude` on PATH — the user's normal launcher. It performs a config
///    sync (auth / endpoint setup, printed as a `同步配置…` line on **stderr**,
///    which `claude_proc` drains to the log rather than surfacing as an error)
///    then delegates to `claude`, forwarding every argument verbatim. Spawning
///    it makes chat use the exact launch path the user uses interactively, so
///    authentication and config match — a bare `claude` skips that sync and can
///    fail with 401.
/// 3. `claude` on PATH — fallback when `reclaude` is not installed.
///
/// PATH-only scanning is enough for production users (these launchers add
/// themselves to PATH); returns `None` when nothing is found so the caller can
/// surface a clear setup error.
fn resolve_chat_launcher_path() -> Option<String> {
    if let Some(override_path) = std::env::var_os("AIO_CHAT_CLAUDE_LAUNCHER") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ".ps1", ""]
    } else {
        &[""]
    };
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    // `reclaude` is preferred over `claude` across all PATH directories.
    for base in ["reclaude", "claude"] {
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

/// Trim a `permission_mode` string and treat blank/empty as "unset" so the
/// process module can simply omit the `--permission-mode` flag.
fn normalize_permission_mode(mode: Option<String>) -> Option<String> {
    mode.map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
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
}
