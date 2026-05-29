//! Usage: Thin IPC wrappers for M0 chat session commands.
//!
//! These mirror the service entry points in [`crate::app::chat_service`]
//! one-for-one — the heavy lifting (per-session `claude` process lifecycle,
//! validation, event emission) lives there.
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
    /// `claude` permission mode (e.g. `default`, `acceptEdits`, `plan`,
    /// `bypassPermissions`). `None` lets `claude` use its own default.
    pub permission_mode: Option<String>,
    /// Tools to pre-allow for this session (`--allowedTools`). The coarse
    /// allow/ask/deny rules still live in `settings.json` and are managed by
    /// the CLI manager page; this is the per-session overlay only.
    pub allowed_tools: Option<Vec<String>>,
    /// Tools to deny for this session (`--disallowedTools`).
    pub disallowed_tools: Option<Vec<String>>,
    /// Which launcher to start the session with: `"reclaude"` or `"claude"`.
    /// `None` (or any other value) means auto — prefer `reclaude`, fall back
    /// to `claude` (honouring the `AIO_CHAT_CLAUDE_LAUNCHER` override).
    pub launcher: Option<String>,
    /// Model alias or id for `--model` (e.g. `claude-sonnet-4-6`, `opus`).
    /// Blank/`None` lets `claude` pick its default; the value is validated
    /// upstream by `claude`, not here.
    pub model: Option<String>,
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

#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatListSlashCommandsInput {
    /// Project root used to discover `<cwd>/.claude/commands/*.md`. Pass the
    /// session's cwd (or the default cwd) so project-local commands surface.
    pub cwd: String,
}

/// A slash-command candidate for the frontend `/` autocomplete.
///
/// `source` is `"builtin"`, `"skill"`, or `"command"`. The list is
/// best-effort (see [`chat_service::list_slash_commands`]); the frontend
/// re-validates against the session's authoritative list once it starts.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
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
/// Spawns a dedicated `claude` process for the session and returns its
/// freshly generated session id (UUID v4). Per-session permission knobs are
/// passed straight through; absent tool lists default to empty.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_create_session(
    app: tauri::AppHandle,
    chat_state: tauri::State<'_, ChatState>,
    input: ChatCreateSessionInput,
) -> Result<String, String> {
    chat_service::create_session(
        chat_state.service(),
        app,
        input.cwd,
        input.permission_mode,
        input.allowed_tools.unwrap_or_default(),
        input.disallowed_tools.unwrap_or_default(),
        input.launcher,
        input.model,
    )
    .await
    .map_err(Into::into)
}

/// Forward a user message to an existing chat session.
///
/// The `claude` process's streaming stream-json output is delivered
/// out-of-band as `chat-event-{session_id}` Tauri events.
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

/// List slash-command candidates for the `/` autocomplete, merging built-ins,
/// installed skills, and custom commands (user + `<cwd>/.claude/commands`).
///
/// Best-effort: never errors on a missing/unreadable source — it returns
/// whatever it can gather (always at least the built-ins). `Ok` is therefore
/// the only realistic outcome; the `Result` is kept for IPC symmetry.
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_list_slash_commands(
    app: tauri::AppHandle,
    input: ChatListSlashCommandsInput,
) -> Result<Vec<ChatSlashCommand>, String> {
    let commands = chat_service::list_slash_commands(&app, &input.cwd)
        .into_iter()
        .map(|c| ChatSlashCommand {
            name: c.name,
            description: c.description,
            source: c.source.to_string(),
        })
        .collect();
    Ok(commands)
}
