//! Usage: Thin IPC wrappers for M0 chat session commands.
//!
//! These mirror the service entry points in [`crate::app::chat_service`]
//! one-for-one — the heavy lifting (sidecar lifecycle, validation,
//! event emission) lives there.
//!
//! Convention: every input is wrapped in a `*Input` struct with
//! `#[serde(rename_all = "camelCase")]` to match the rest of the codebase
//! (see e.g. `mcp.rs` / `provider_*` commands). This keeps the frontend
//! JS side talking pure camelCase without per-command `rename_all`
//! attributes on the Tauri command.

use crate::app::chat_service::{self, ChatState};

#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCreateSessionInput {
    pub cwd: String,
}

#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSendMessageInput {
    pub session_id: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCloseSessionInput {
    pub session_id: String,
}

/// Resolve a default cwd for new chat sessions (currently the user's
/// home directory). The frontend calls this because AIO deliberately
/// withholds the Tauri `core:path:*` permissions from the webview, so
/// `homeDir()` etc. cannot run there.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_default_cwd(app: tauri::AppHandle) -> Result<String, String> {
    chat_service::default_cwd(&app).map_err(Into::into)
}

/// Create a new chat session bound to an absolute, existing `cwd`.
///
/// Returns the freshly generated session id (UUID v4) once the sidecar
/// acknowledges the session.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_create_session(
    app: tauri::AppHandle,
    chat_state: tauri::State<'_, ChatState>,
    input: ChatCreateSessionInput,
) -> Result<String, String> {
    chat_service::create_session(chat_state.service(), app, input.cwd)
        .await
        .map_err(Into::into)
}

/// Forward a user message to an existing chat session.
///
/// Streaming SDK output is delivered out-of-band as `chat-event-{session_id}`
/// Tauri events.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_send_message(
    chat_state: tauri::State<'_, ChatState>,
    input: ChatSendMessageInput,
) -> Result<(), String> {
    chat_service::send_message(chat_state.service(), input.session_id, input.content)
        .await
        .map_err(Into::into)
}

/// Tear down a chat session. Idempotent: returns `Ok(())` if the session
/// id is unknown so the frontend can safely retry.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_close_session(
    chat_state: tauri::State<'_, ChatState>,
    input: ChatCloseSessionInput,
) -> Result<(), String> {
    chat_service::close_session(chat_state.service(), input.session_id)
        .await
        .map_err(Into::into)
}
