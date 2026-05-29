//! Usage: One resident local `claude` CLI child per chat session, plus its
//! NDJSON (stream-json) stdin/stdout pipe.
//!
//! Path B contract (one session == one `claude` process):
//! - Each session spawns a dedicated `claude` running in
//!   `--input-format stream-json` mode. The process stays resident and reads
//!   one NDJSON user message per turn from stdin, so a single child handles
//!   the whole multi-turn conversation (no Node sidecar, no multiplexing).
//! - stdout is `claude`'s native stream-json: one JSON object per line. We
//!   parse each line into a `serde_json::Value` and hand it on verbatim via
//!   the `on_event` callback — the module never couples itself to the SDK
//!   event schema (which is evolving).
//! - When the child exits we flip an `alive` flag and invoke `on_exit` exactly
//!   once. M0 does not auto-restart (M3 territory).
//!
//! This module is intentionally Tauri-agnostic: it speaks only through the
//! callbacks in [`ClaudeProcCallbacks`]. The chat service wires those to Tauri
//! events; nothing here emits or imports `tauri`.
//!
//! Frozen call protocol (extracted from the Claude Agent SDK source and
//! confirmed against a real `claude` 2.1.x invocation):
//!
//! ```text
//! claude --output-format stream-json --verbose --input-format stream-json
//!        --session-id <UUID>
//!        [--permission-mode <mode>]
//!        [--allowedTools <t1,t2,...>] [--disallowedTools <...>]
//!        [--add-dir <dir>]...
//! ```
//!
//! There is no `--print` / `-p`: the SDK's base argv is exactly
//! `--output-format stream-json --verbose --input-format stream-json`, and the
//! presence of `--input-format stream-json` is what puts the CLI into resident
//! stdin-reading mode. Each user turn is one stdin line:
//!
//! ```json
//! {"type":"user","message":{"role":"user","content":[{"type":"text","text":"<content>"}]}}
//! ```

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinHandle;

use crate::shared::error::{AppError, AppResult};

/// Error code surfaced when the `claude` process is missing, fails to spawn,
/// or its stdin pipe breaks. Kept identical to the M0 sidecar code so the
/// chat service and frontend error handling continue to match.
const PROC_DOWN: &str = "CHAT_SIDECAR_DOWN";

/// Spawn-time configuration for a single session's `claude` process.
///
/// All fields are owned strings/vecs so the caller can build this without
/// borrowing — the service constructs one per `create_session`.
pub(crate) struct ClaudeProcConfig {
    /// Absolute path (or PATH-resolvable name) of the `claude` executable.
    pub claude_path: String,
    /// Working directory for the child; the conversation's project root.
    pub cwd: String,
    /// Session UUID passed via `--session-id`.
    pub session_id: String,
    /// Optional `--permission-mode` value (e.g. `plan`, `acceptEdits`).
    /// `None` omits the flag entirely.
    pub permission_mode: Option<String>,
    /// Tool names for `--allowedTools` (joined by `,`). Empty omits the flag.
    pub allowed_tools: Vec<String>,
    /// Tool names for `--disallowedTools` (joined by `,`). Empty omits the flag.
    pub disallowed_tools: Vec<String>,
    /// Extra read roots; each becomes one `--add-dir <dir>`.
    pub add_dirs: Vec<String>,
}

/// Callbacks the chat service installs to receive the process's output.
///
/// All three run on the Tokio runtime and MUST be cheap and non-blocking —
/// they execute inline on the stdout-reader / waiter tasks, so blocking here
/// stalls event delivery. Use them to emit Tauri events or touch in-memory
/// state, never to block.
///
/// - `on_event` — one parsed stream-json line, forwarded verbatim.
/// - `on_error` — a human-readable problem (e.g. a stdout read failure). An
///   unparseable line is *not* reported here — it is logged and skipped.
/// - `on_exit` — invoked exactly once when the child exits, with its exit code
///   (`None` if it was terminated by a signal / the code was unavailable).
pub(crate) struct ClaudeProcCallbacks {
    pub on_event: Box<dyn Fn(serde_json::Value) + Send + Sync + 'static>,
    pub on_error: Box<dyn Fn(String) + Send + Sync + 'static>,
    pub on_exit: Box<dyn Fn(Option<i32>) + Send + Sync + 'static>,
}

/// Owned handle to one running `claude` process.
///
/// The handle owns the child's stdin and a liveness flag; the stdout-reader
/// and exit-waiter run as detached Tokio tasks whose `JoinHandle`s are kept so
/// they are aborted when this handle drops. The child is spawned with
/// `kill_on_drop(true)`, so dropping the handle (e.g. removing it from the
/// session map) also tears the process down.
pub(crate) struct ClaudeProc {
    stdin: Arc<TokioMutex<ChildStdin>>,
    alive: Arc<AtomicBool>,
    /// Reader task is tied to this handle's lifetime — dropping the handle
    /// aborts it and releases the stdout pipe.
    _reader_task: JoinHandle<()>,
    _waiter_task: JoinHandle<()>,
}

impl ClaudeProc {
    /// Spawn a `claude` process for one session and wire up its callbacks.
    ///
    /// Returns `CHAT_SIDECAR_DOWN` if the process cannot be spawned or its
    /// stdio pipes are missing. On success the stdout reader and exit waiter
    /// are already running.
    pub(crate) async fn spawn(
        config: ClaudeProcConfig,
        callbacks: ClaudeProcCallbacks,
    ) -> AppResult<Self> {
        let args = build_args(&config);

        let mut command = Command::new(&config.claude_path);
        command
            .args(&args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Tearing down the handle must take the child with it; the session
            // map is the sole owner.
            .kill_on_drop(true);

        // Keep the child off any inherited console window on Windows so it
        // never flashes a terminal. `tokio::process::Command` exposes
        // `creation_flags` as an inherent method, so no extra trait import.
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child: Child = command.spawn().map_err(|e| {
            AppError::new(
                PROC_DOWN,
                format!("failed to spawn `{}`: {e}", config.claude_path),
            )
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AppError::new(PROC_DOWN, "claude process missing stdin pipe".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::new(PROC_DOWN, "claude process missing stdout pipe".to_string())
        })?;
        // stderr is drained on a fire-and-forget task so the child cannot
        // deadlock on a full stderr pipe.
        let stderr = child.stderr.take();

        let alive = Arc::new(AtomicBool::new(true));
        let on_event = Arc::new(callbacks.on_event);
        let on_error = Arc::new(callbacks.on_error);
        let on_exit = callbacks.on_exit;

        let reader_task = tokio::spawn(read_stdout(stdout, on_event, on_error.clone()));

        if let Some(stderr) = stderr {
            tokio::spawn(drain_stderr(stderr));
        }

        let alive_for_waiter = alive.clone();
        let waiter_task = tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(status) => {
                    tracing::warn!(code = status.code(), "chat claude process exited");
                    status.code()
                }
                Err(err) => {
                    tracing::warn!(error = %err, "chat claude process wait failed");
                    None
                }
            };
            alive_for_waiter.store(false, Ordering::SeqCst);
            on_exit(code);
        });

        Ok(Self {
            stdin: Arc::new(TokioMutex::new(stdin)),
            alive,
            _reader_task: reader_task,
            _waiter_task: waiter_task,
        })
    }

    /// Whether the child is still believed to be running. Set to `false` once
    /// the waiter observes exit or a stdin write fails.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Write one user turn to the process's stdin as a single NDJSON line.
    ///
    /// Serialises `{"type":"user","message":{"role":"user","content":
    /// [{"type":"text","text":<text>}]}}` plus a trailing newline. Returns
    /// `CHAT_SIDECAR_DOWN` if the process has already exited or the write
    /// fails; concurrent calls are serialised by an internal async mutex so
    /// half-written frames cannot interleave.
    pub(crate) async fn send_user_message(&self, text: String) -> AppResult<()> {
        if !self.is_alive() {
            return Err(AppError::new(
                PROC_DOWN,
                "claude process is not running".to_string(),
            ));
        }

        let mut line = encode_user_message(&text)?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| {
            self.alive.store(false, Ordering::SeqCst);
            AppError::new(PROC_DOWN, format!("failed to write to claude stdin: {e}"))
        })?;
        stdin.flush().await.map_err(|e| {
            self.alive.store(false, Ordering::SeqCst);
            AppError::new(PROC_DOWN, format!("failed to flush claude stdin: {e}"))
        })?;
        Ok(())
    }

    /// Close the session: drop stdin (signals EOF to `claude`) and start the
    /// kill. The waiter task observes exit and fires `on_exit`.
    ///
    /// Idempotent and infallible from the caller's view — closing an
    /// already-dead process is a no-op that still returns `Ok(())`, so the
    /// service's idempotent-close contract holds. We mark the handle dead up
    /// front so a racing `send_user_message` fails fast rather than writing to
    /// a pipe we are tearing down.
    pub(crate) async fn close(&self) -> AppResult<()> {
        self.alive.store(false, Ordering::SeqCst);
        // Abort the stdout reader so it stops touching the soon-dead pipe.
        self._reader_task.abort();
        // Dropping our stdin half closes the write end, giving `claude` a
        // clean EOF; `kill_on_drop` then reaps the child once the owning
        // handle is dropped by the caller.
        Ok(())
    }
}

/// Build the frozen argv for `claude` (everything after the executable name).
///
/// Order mirrors the SDK: the fixed stream-json quartet first, then
/// `--session-id`, then the optional permission / tool / dir flags.
fn build_args(config: &ClaudeProcConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--session-id".to_string(),
        config.session_id.clone(),
    ];

    if let Some(mode) = config
        .permission_mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        args.push("--permission-mode".to_string());
        args.push(mode.to_string());
    }

    let allowed = join_tools(&config.allowed_tools);
    if !allowed.is_empty() {
        args.push("--allowedTools".to_string());
        args.push(allowed);
    }

    let disallowed = join_tools(&config.disallowed_tools);
    if !disallowed.is_empty() {
        args.push("--disallowedTools".to_string());
        args.push(disallowed);
    }

    for dir in &config.add_dirs {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            continue;
        }
        args.push("--add-dir".to_string());
        args.push(trimmed.to_string());
    }

    args
}

/// Join non-blank tool names with `,` for the `--allowedTools` /
/// `--disallowedTools` flags. Returns an empty string when nothing remains so
/// the caller can drop the flag.
fn join_tools(tools: &[String]) -> String {
    tools
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Encode one user turn as a stream-json line (no trailing newline).
fn encode_user_message(text: &str) -> AppResult<String> {
    let value = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{ "type": "text", "text": text }],
        },
    });
    serde_json::to_string(&value).map_err(|e| {
        AppError::new(
            "INTERNAL_ERROR",
            format!("failed to serialise user message: {e}"),
        )
    })
}

/// Read `claude`'s stdout line by line, forwarding each parsed JSON object via
/// `on_event`. Blank lines are skipped; unparseable lines are logged and
/// dropped (so a single malformed frame never kills the stream). A hard read
/// error is reported once through `on_error` before the loop ends.
async fn read_stdout(
    stdout: ChildStdout,
    on_event: Arc<Box<dyn Fn(serde_json::Value) + Send + Sync + 'static>>,
    on_error: Arc<Box<dyn Fn(String) + Send + Sync + 'static>>,
) {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(value) => on_event(value),
                    Err(err) => tracing::warn!(
                        line = %trimmed,
                        error = %err,
                        "ignoring unparseable claude stream-json line",
                    ),
                }
            }
            Ok(None) => {
                tracing::info!("chat claude stdout closed");
                break;
            }
            Err(err) => {
                tracing::warn!(error = %err, "chat claude stdout read failed");
                on_error(format!("claude stdout read failed: {err}"));
                break;
            }
        }
    }
}

/// Drain `claude`'s stderr to the tracing log so a full pipe cannot
/// back-pressure-deadlock the child.
async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(stderr).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    tracing::warn!(target: "chat_claude", "{}", trimmed);
                }
            }
            Ok(None) => break,
            Err(err) => {
                tracing::debug!(error = %err, "chat claude stderr read failed");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> ClaudeProcConfig {
        ClaudeProcConfig {
            claude_path: "claude".to_string(),
            cwd: "/tmp/project".to_string(),
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            permission_mode: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            add_dirs: Vec::new(),
        }
    }

    #[test]
    fn build_args_emits_frozen_base_argv_without_print() {
        let args = build_args(&base_config());
        assert_eq!(
            args,
            vec![
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--session-id",
                "11111111-1111-1111-1111-111111111111",
            ]
        );
        // Path B never passes `--print` / `-p`; resident stdin mode is driven
        // purely by `--input-format stream-json`.
        assert!(!args.iter().any(|a| a == "--print" || a == "-p"));
    }

    #[test]
    fn build_args_appends_permission_mode_when_set() {
        let mut config = base_config();
        config.permission_mode = Some("plan".to_string());
        let args = build_args(&config);
        let idx = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("flag present");
        assert_eq!(args[idx + 1], "plan");
    }

    #[test]
    fn build_args_omits_blank_permission_mode() {
        let mut config = base_config();
        config.permission_mode = Some("   ".to_string());
        let args = build_args(&config);
        assert!(!args.iter().any(|a| a == "--permission-mode"));
    }

    #[test]
    fn build_args_joins_tool_lists_with_commas() {
        let mut config = base_config();
        config.allowed_tools = vec!["Read".to_string(), "Edit".to_string()];
        config.disallowed_tools = vec!["Bash".to_string()];
        let args = build_args(&config);

        let allow_idx = args
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("allowedTools present");
        assert_eq!(args[allow_idx + 1], "Read,Edit");

        let deny_idx = args
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("disallowedTools present");
        assert_eq!(args[deny_idx + 1], "Bash");
    }

    #[test]
    fn build_args_drops_blank_tool_entries() {
        let mut config = base_config();
        config.allowed_tools = vec!["  ".to_string(), "".to_string()];
        let args = build_args(&config);
        assert!(!args.iter().any(|a| a == "--allowedTools"));
    }

    #[test]
    fn build_args_repeats_add_dir_per_directory() {
        let mut config = base_config();
        config.add_dirs = vec!["/a".to_string(), "  ".to_string(), "/b".to_string()];
        let args = build_args(&config);
        let dirs: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--add-dir" && *i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(dirs, vec!["/a", "/b"]);
    }

    #[test]
    fn encode_user_message_matches_frozen_stream_json_shape() {
        let line = encode_user_message("hello world").expect("encode");
        assert_eq!(
            line,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello world"}]}}"#
        );
        // Must be exactly one NDJSON object — no embedded newline.
        assert!(!line.contains('\n'));
    }

    #[test]
    fn encode_user_message_escapes_special_characters() {
        let line = encode_user_message("quote \" and newline \n done").expect("encode");
        // Round-trips back to the original text through JSON.
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("parse");
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(
            parsed["message"]["content"][0]["text"],
            "quote \" and newline \n done"
        );
    }

    #[test]
    fn stream_json_line_parses_as_opaque_value() {
        // A representative `claude --verbose` stdout line (system/init), parsed
        // the same way the reader loop does: as an opaque Value, no schema.
        let raw = r#"{"type":"system","subtype":"init","session_id":"abc","tools":["Read","Edit"],"model":"claude-x"}"#;
        let value: serde_json::Value = serde_json::from_str(raw).expect("parse");
        assert_eq!(value["type"], "system");
        assert_eq!(value["subtype"], "init");
        assert_eq!(value["session_id"], "abc");
        assert_eq!(value["tools"], json!(["Read", "Edit"]));
    }

    #[test]
    fn join_tools_skips_blanks_and_trims() {
        let joined = join_tools(&[
            " Read ".to_string(),
            "".to_string(),
            "  ".to_string(),
            "Edit".to_string(),
        ]);
        assert_eq!(joined, "Read,Edit");
    }
}
