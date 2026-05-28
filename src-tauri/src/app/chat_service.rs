//! Usage: M0 chat service — owns the Node sidecar handle and chat session state.
//!
//! Responsibilities:
//! - Lazily spawn the sidecar on first `create_session`; reuse it afterwards.
//! - Track in-memory session state keyed by UUID; M0 keeps everything in
//!   RAM (no DB), wiped on app exit.
//! - Translate sidecar stdout into Tauri events targeted per session.
//! - Validate session cwd (absolute + existing + not the AIO data dir).
//!
//! Out of scope (later milestones):
//! - DB persistence and migration v33 (M2).
//! - Sidecar auto-restart and health checks (M3).
//! - Production-mode bundled sidecar path resolution (M4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rand::RngCore;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::{oneshot, Mutex as TokioMutex};

use crate::infra::sidecar::{Sidecar, SidecarCallbacks, SidecarRequest, SidecarResponse};
use crate::shared::error::{AppError, AppResult};

/// Default Node executable name when the host PATH already resolves it.
const DEFAULT_NODE_EXECUTABLE: &str = "node";

/// Override env var: absolute path to a `node` binary. Useful in CI and
/// for devs whose PATH does not include Node.
const NODE_OVERRIDE_ENV: &str = "AIO_CHAT_NODE_PATH";

/// In-memory record of a live session.
///
/// M0 keeps only the minimum needed to honour follow-up commands; the
/// `pending_ready` channel lets `create_session` wait for the sidecar's
/// `session_ready` acknowledgement before returning to the frontend.
struct SessionState {
    #[allow(dead_code)] // surfaced through diagnostics in later milestones.
    cwd: PathBuf,
    pending_ready: Option<oneshot::Sender<Result<(), String>>>,
}

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
#[derive(Default)]
pub(crate) struct ChatService {
    sidecar: TokioMutex<Option<Arc<Sidecar>>>,
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl ChatService {
    fn has_session(&self, session_id: &str) -> bool {
        match lock_sessions(&self.sessions) {
            Ok(sessions) => sessions.contains_key(session_id),
            Err(_) => false,
        }
    }

    fn remove_session(&self, session_id: &str) {
        if let Ok(mut sessions) = lock_sessions(&self.sessions) {
            sessions.remove(session_id);
        }
    }

    /// Settle the `session_ready` waiter, if one is registered.
    fn settle_pending_ready(&self, session_id: &str, outcome: Result<(), String>) {
        let waiter = {
            let Ok(mut sessions) = lock_sessions(&self.sessions) else {
                return;
            };
            sessions
                .get_mut(session_id)
                .and_then(|session| session.pending_ready.take())
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send(outcome);
        }
    }

    /// Wake every pending waiter with a sidecar-down error.
    fn mark_all_sidecar_down(&self) {
        if let Ok(mut sessions) = lock_sessions(&self.sessions) {
            for (id, session) in sessions.iter_mut() {
                if let Some(waiter) = session.pending_ready.take() {
                    let _ = waiter.send(Err(format!("sidecar exited before {id} was ready")));
                }
            }
        }
    }

    /// Look up the cached sidecar handle (without spawning).
    async fn current_sidecar(&self) -> Option<Arc<Sidecar>> {
        let guard = self.sidecar.lock().await;
        guard.as_ref().filter(|s| s.is_alive()).cloned()
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatErrorPayload {
    error: String,
}

/// Create a chat session bound to `cwd`.
///
/// 1. Canonicalises `cwd` and rejects anything that is not an existing
///    directory or that lives inside the AIO data dir.
/// 2. Lazily spawns the sidecar (subsequent calls reuse the handle).
/// 3. Sends `create_session` and awaits the matching `session_ready`
///    response before returning the new session id.
pub(crate) async fn create_session<R: tauri::Runtime>(
    service: Arc<ChatService>,
    app: tauri::AppHandle<R>,
    cwd: String,
) -> AppResult<String> {
    let cwd = validate_cwd(&app, &cwd)?;
    let session_id = generate_uuid_v4();
    let (ready_tx, ready_rx) = oneshot::channel();

    {
        let mut sessions = lock_sessions(&service.sessions)?;
        sessions.insert(
            session_id.clone(),
            SessionState {
                cwd: cwd.clone(),
                pending_ready: Some(ready_tx),
            },
        );
    }

    let sidecar = match ensure_sidecar(&service, &app).await {
        Ok(sidecar) => sidecar,
        Err(err) => {
            service.remove_session(&session_id);
            return Err(err);
        }
    };

    let request = SidecarRequest::CreateSession {
        session_id: session_id.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
    };
    if let Err(err) = sidecar.send(&request).await {
        service.remove_session(&session_id);
        return Err(err);
    }

    match ready_rx.await {
        Ok(Ok(())) => Ok(session_id),
        Ok(Err(reason)) => {
            service.remove_session(&session_id);
            Err(AppError::new(
                "CHAT_SESSION_NOT_FOUND",
                format!("sidecar refused session: {reason}"),
            ))
        }
        Err(_) => {
            service.remove_session(&session_id);
            Err(AppError::new(
                "CHAT_SIDECAR_DOWN",
                "sidecar dropped before session_ready".to_string(),
            ))
        }
    }
}

pub(crate) async fn send_message(
    service: Arc<ChatService>,
    session_id: String,
    content: String,
) -> AppResult<()> {
    if !service.has_session(&session_id) {
        return Err(AppError::new(
            "CHAT_SESSION_NOT_FOUND",
            format!("session {session_id} not found"),
        ));
    }

    let sidecar = service.current_sidecar().await.ok_or_else(|| {
        AppError::new(
            "CHAT_SIDECAR_DOWN",
            "sidecar has not been started".to_string(),
        )
    })?;
    sidecar
        .send(&SidecarRequest::SendMessage {
            session_id,
            content,
        })
        .await
}

pub(crate) async fn close_session(service: Arc<ChatService>, session_id: String) -> AppResult<()> {
    if !service.has_session(&session_id) {
        // Closing an already-closed session is a no-op — the frontend may
        // retry after a crash or reload. Return Ok so it does not raise.
        return Ok(());
    }

    let sidecar = service.current_sidecar().await;
    let send_result = if let Some(sidecar) = sidecar {
        sidecar
            .send(&SidecarRequest::CloseSession {
                session_id: session_id.clone(),
            })
            .await
    } else {
        Ok(())
    };

    service.remove_session(&session_id);
    send_result
}

/// Resolve a sensible default cwd for a new chat session.
///
/// Returns the user's home directory as an absolute path. The frontend
/// calls this through the `chat_default_cwd` IPC command — AIO does not
/// expose the Tauri `core:path:*` permissions to the webview by design
/// (see `capabilities/main-core.json`), so all path resolution happens
/// in Rust. M1 will replace this with a per-session cwd picker.
pub(crate) fn default_cwd<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<String> {
    let home = crate::app_paths::home_dir(app)?;
    Ok(home.to_string_lossy().into_owned())
}

async fn ensure_sidecar<R: tauri::Runtime>(
    service: &Arc<ChatService>,
    app: &tauri::AppHandle<R>,
) -> AppResult<Arc<Sidecar>> {
    let mut guard = service.sidecar.lock().await;
    if let Some(existing) = guard.as_ref() {
        if existing.is_alive() {
            return Ok(existing.clone());
        }
        // Previous instance died — drop it before re-spawning.
        *guard = None;
    }

    let script_path = sidecar_script_path()?;
    let node_executable = resolve_node_executable();
    let callbacks = build_callbacks(service.clone(), app);
    let sidecar = Arc::new(Sidecar::spawn(&node_executable, script_path, callbacks).await?);
    *guard = Some(sidecar.clone());
    Ok(sidecar)
}

fn build_callbacks<R: tauri::Runtime>(
    service: Arc<ChatService>,
    app: &tauri::AppHandle<R>,
) -> SidecarCallbacks {
    let app_for_response = app.clone();
    let app_for_exit = app.clone();
    let service_weak = Arc::downgrade(&service);

    let response_service = service_weak.clone();
    let on_response = move |response: SidecarResponse| {
        let Some(service) = response_service.upgrade() else {
            return;
        };
        match response {
            SidecarResponse::Ready {
                sidecar_version,
                sdk_version,
            } => {
                tracing::info!(
                    sidecar_version = %sidecar_version,
                    sdk_version = %sdk_version,
                    "chat sidecar ready",
                );
            }
            SidecarResponse::Pong => {}
            SidecarResponse::SessionReady { session_id } => {
                service.settle_pending_ready(&session_id, Ok(()));
            }
            SidecarResponse::Event {
                session_id,
                sdk_event,
            } => {
                let event_name = format!("chat-event-{session_id}");
                if let Err(err) = app_for_response.emit(&event_name, sdk_event) {
                    tracing::warn!(
                        event = %event_name,
                        error = %err,
                        "failed to emit chat event",
                    );
                }
            }
            SidecarResponse::SessionError { session_id, error } => {
                // If create_session is still waiting, surface the error there;
                // otherwise also emit a chat-error-* event so the frontend can
                // surface it on the running session.
                service.settle_pending_ready(&session_id, Err(error.clone()));
                let event_name = format!("chat-error-{session_id}");
                let payload = ChatErrorPayload { error };
                if let Err(err) = app_for_response.emit(&event_name, payload) {
                    tracing::warn!(
                        event = %event_name,
                        error = %err,
                        "failed to emit chat error",
                    );
                }
            }
        }
    };

    let on_exit = move || {
        if let Some(service) = service_weak.upgrade() {
            service.mark_all_sidecar_down();
        }
        if let Err(err) = app_for_exit.emit("chat-sidecar-exited", ()) {
            tracing::debug!(error = %err, "failed to emit chat-sidecar-exited");
        }
    };

    SidecarCallbacks {
        on_response: Box::new(on_response),
        on_exit: Box::new(on_exit),
    }
}

fn lock_sessions(
    sessions: &Mutex<HashMap<String, SessionState>>,
) -> AppResult<std::sync::MutexGuard<'_, HashMap<String, SessionState>>> {
    sessions
        .lock()
        .map_err(|_| AppError::new("INTERNAL_ERROR", "chat session map poisoned".to_string()))
}

fn resolve_node_executable() -> String {
    std::env::var(NODE_OVERRIDE_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_NODE_EXECUTABLE.to_string())
}

fn sidecar_script_path() -> AppResult<PathBuf> {
    // Dev mode resolution: <repo>/src-tauri/sidecar/chat-bridge/dist/chat-bridge.js.
    // `CARGO_MANIFEST_DIR` resolves to `<repo>/src-tauri` for this crate.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        AppError::new(
            "CHAT_SIDECAR_DOWN",
            "CARGO_MANIFEST_DIR is unset; cannot locate sidecar".to_string(),
        )
    })?;
    let path = PathBuf::from(manifest_dir)
        .join("sidecar")
        .join("chat-bridge")
        .join("dist")
        .join("chat-bridge.js");
    Ok(path)
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
    fn sidecar_script_path_uses_manifest_dir() {
        // CARGO_MANIFEST_DIR is always set during `cargo test`, so this
        // really just verifies the suffix is what we expect.
        let path = sidecar_script_path().expect("manifest dir is set during tests");
        let suffix = Path::new("sidecar")
            .join("chat-bridge")
            .join("dist")
            .join("chat-bridge.js");
        assert!(
            path.ends_with(&suffix),
            "expected path to end with {suffix:?}, got {path:?}",
        );
    }

    #[test]
    fn resolve_node_executable_honours_env_override() {
        let key = NODE_OVERRIDE_ENV;
        let previous = env::var(key).ok();
        env::set_var(key, "/custom/node");
        assert_eq!(resolve_node_executable(), "/custom/node");

        env::set_var(key, "  ");
        assert_eq!(resolve_node_executable(), DEFAULT_NODE_EXECUTABLE);

        env::remove_var(key);
        assert_eq!(resolve_node_executable(), DEFAULT_NODE_EXECUTABLE);

        if let Some(value) = previous {
            env::set_var(key, value);
        }
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
