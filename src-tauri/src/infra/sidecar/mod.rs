//! Usage: Node `chat-bridge` sidecar process lifecycle and NDJSON pipe.
//!
//! M0 contract:
//! - Single long-lived Node process; lazily spawned on first chat command.
//! - One stdin/stdout pipe pair carrying newline-delimited JSON.
//! - Multiple chat sessions multiplex over the same pipe (sidecar isolates
//!   them via `session_id`).
//! - When the child exits, we flip an `alive` flag; subsequent commands
//!   surface `CHAT_SIDECAR_DOWN` instead of dialing a dead pipe. M0 does
//!   not auto-restart (M3 territory).
//!
//! Production packaging (M4) will need to swap the dev path for the
//! bundled resource path — for now we hard-code the in-repo dist path.
//
// TODO(M4): replace dev-only sidecar path resolution with a bundled
// resource lookup via `tauri::path::resolve_resource`.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinHandle;

use crate::shared::error::{AppError, AppResult};

/// Inbound message — Rust writes these to the sidecar's stdin.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SidecarRequest {
    #[allow(dead_code)] // ping is reserved for future health probing.
    Ping,
    CreateSession {
        session_id: String,
        cwd: String,
    },
    SendMessage {
        session_id: String,
        content: String,
    },
    CloseSession {
        session_id: String,
    },
}

/// Outbound message — Rust parses these from the sidecar's stdout.
///
/// `event` arrives with a raw SDK payload; we hand it on as
/// `serde_json::Value` so we do not couple the gateway to the SDK schema
/// (which is evolving).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SidecarResponse {
    Ready {
        sidecar_version: String,
        sdk_version: String,
    },
    #[allow(dead_code)] // pong is reserved for future health probing.
    Pong,
    SessionReady {
        session_id: String,
    },
    Event {
        session_id: String,
        sdk_event: serde_json::Value,
    },
    SessionError {
        session_id: String,
        error: String,
    },
}

/// Callbacks the chat service installs to receive sidecar output.
///
/// Both callbacks run on the Tokio runtime; they MUST be cheap and
/// non-blocking. Use them to emit Tauri events or write to in-memory
/// state, never to block the stdout loop.
pub(crate) struct SidecarCallbacks {
    pub(crate) on_response: Box<dyn Fn(SidecarResponse) + Send + Sync + 'static>,
    pub(crate) on_exit: Box<dyn Fn() + Send + Sync + 'static>,
}

/// Owned handle to a running sidecar process.
///
/// Cloning produces a new handle that shares the same child, pipe and
/// liveness flag — the `Arc` boundary is internal so the service layer
/// can stash an `Arc<Sidecar>` and clone it freely.
pub(crate) struct Sidecar {
    stdin: Arc<TokioMutex<ChildStdin>>,
    alive: Arc<AtomicBool>,
    /// Kept so the reader task is tied to the sidecar's lifetime (drops
    /// abort the task, releasing the stdout pipe).
    _reader_task: JoinHandle<()>,
    _waiter_task: JoinHandle<()>,
}

impl Sidecar {
    /// Spawn the Node sidecar at `node_path script_path`.
    ///
    /// The reader task installs the supplied callbacks; the waiter task
    /// flips `alive` to `false` when the child exits and invokes
    /// `on_exit` exactly once.
    pub(crate) async fn spawn(
        node_path: &str,
        script_path: PathBuf,
        callbacks: SidecarCallbacks,
    ) -> AppResult<Self> {
        if !script_path.is_file() {
            return Err(AppError::new(
                "CHAT_SIDECAR_DOWN",
                format!(
                    "sidecar script not found at {} (build chat-bridge first)",
                    script_path.display()
                ),
            ));
        }

        let mut command = Command::new(node_path);
        command
            .arg(&script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Detach the child's stderr/stdout from any inherited console
            // — we manage them through pipes.
            .kill_on_drop(true);

        // Run in the script's own directory so relative `require()` and
        // sourcemap lookups behave the same as `node dist/chat-bridge.js`
        // from the repo root.
        if let Some(parent) = script_path.parent() {
            command.current_dir(parent);
        }

        let mut child: Child = command.spawn().map_err(|e| {
            AppError::new(
                "CHAT_SIDECAR_DOWN",
                format!("failed to spawn sidecar `{node_path}`: {e}"),
            )
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AppError::new(
                "CHAT_SIDECAR_DOWN",
                "sidecar missing stdin pipe".to_string(),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::new(
                "CHAT_SIDECAR_DOWN",
                "sidecar missing stdout pipe".to_string(),
            )
        })?;
        // stderr is read on a fire-and-forget task so the child does not
        // back-pressure-deadlock on a full pipe.
        let stderr = child.stderr.take();

        let alive = Arc::new(AtomicBool::new(true));
        let on_response = Arc::new(callbacks.on_response);
        let on_exit = Arc::new(callbacks.on_exit);

        let reader_task = tokio::spawn(read_stdout(stdout, on_response));

        if let Some(stderr) = stderr {
            tokio::spawn(drain_stderr(stderr));
        }

        let alive_for_waiter = alive.clone();
        let on_exit_for_waiter = on_exit.clone();
        let waiter_task = tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => tracing::warn!(code = status.code(), "chat sidecar exited"),
                Err(err) => tracing::warn!(error = %err, "chat sidecar wait failed"),
            }
            alive_for_waiter.store(false, Ordering::SeqCst);
            on_exit_for_waiter();
        });

        Ok(Self {
            stdin: Arc::new(TokioMutex::new(stdin)),
            alive,
            _reader_task: reader_task,
            _waiter_task: waiter_task,
        })
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Serialise `request` as one NDJSON line and write it to stdin.
    ///
    /// Returns `CHAT_SIDECAR_DOWN` if the sidecar has already exited or
    /// the write fails. Concurrent calls are serialised by an internal
    /// async mutex so half-written frames cannot interleave.
    pub(crate) async fn send(&self, request: &SidecarRequest) -> AppResult<()> {
        if !self.is_alive() {
            return Err(AppError::new(
                "CHAT_SIDECAR_DOWN",
                "sidecar process is not running".to_string(),
            ));
        }

        let mut line = serde_json::to_string(request).map_err(|e| {
            AppError::new(
                "INTERNAL_ERROR",
                format!("failed to serialise sidecar request: {e}"),
            )
        })?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| {
            self.alive.store(false, Ordering::SeqCst);
            AppError::new(
                "CHAT_SIDECAR_DOWN",
                format!("failed to write to sidecar stdin: {e}"),
            )
        })?;
        stdin.flush().await.map_err(|e| {
            self.alive.store(false, Ordering::SeqCst);
            AppError::new(
                "CHAT_SIDECAR_DOWN",
                format!("failed to flush sidecar stdin: {e}"),
            )
        })?;
        Ok(())
    }
}

async fn read_stdout(
    stdout: ChildStdout,
    on_response: Arc<Box<dyn Fn(SidecarResponse) + Send + Sync + 'static>>,
) {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<SidecarResponse>(trimmed) {
                    Ok(message) => on_response(message),
                    Err(err) => tracing::warn!(
                        line = %trimmed,
                        error = %err,
                        "ignoring unparseable sidecar line",
                    ),
                }
            }
            Ok(None) => {
                tracing::info!("chat sidecar stdout closed");
                break;
            }
            Err(err) => {
                tracing::warn!(error = %err, "chat sidecar stdout read failed");
                break;
            }
        }
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(stderr).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    tracing::warn!(target: "chat_sidecar", "{}", trimmed);
                }
            }
            Ok(None) => break,
            Err(err) => {
                tracing::debug!(error = %err, "chat sidecar stderr read failed");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_request_serialises_with_snake_case_type() {
        let request = SidecarRequest::CreateSession {
            session_id: "abc".to_string(),
            cwd: "/tmp".to_string(),
        };
        let encoded = serde_json::to_string(&request).expect("serialise");
        assert_eq!(
            encoded,
            r#"{"type":"create_session","session_id":"abc","cwd":"/tmp"}"#
        );
    }

    #[test]
    fn send_message_request_serialises_with_snake_case_type() {
        let request = SidecarRequest::SendMessage {
            session_id: "abc".to_string(),
            content: "hello".to_string(),
        };
        let encoded = serde_json::to_string(&request).expect("serialise");
        assert_eq!(
            encoded,
            r#"{"type":"send_message","session_id":"abc","content":"hello"}"#
        );
    }

    #[test]
    fn close_session_request_serialises_with_snake_case_type() {
        let request = SidecarRequest::CloseSession {
            session_id: "abc".to_string(),
        };
        let encoded = serde_json::to_string(&request).expect("serialise");
        assert_eq!(encoded, r#"{"type":"close_session","session_id":"abc"}"#);
    }

    #[test]
    fn ready_response_deserialises_from_sidecar_payload() {
        let raw = r#"{"type":"ready","sidecar_version":"0.1.0","sdk_version":"1.2.3"}"#;
        let parsed: SidecarResponse = serde_json::from_str(raw).expect("parse");
        match parsed {
            SidecarResponse::Ready {
                sidecar_version,
                sdk_version,
            } => {
                assert_eq!(sidecar_version, "0.1.0");
                assert_eq!(sdk_version, "1.2.3");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn session_ready_response_deserialises() {
        let raw = r#"{"type":"session_ready","session_id":"sess-1"}"#;
        let parsed: SidecarResponse = serde_json::from_str(raw).expect("parse");
        match parsed {
            SidecarResponse::SessionReady { session_id } => assert_eq!(session_id, "sess-1"),
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    #[test]
    fn event_response_preserves_arbitrary_sdk_payload() {
        let raw = r#"{"type":"event","session_id":"s","sdk_event":{"foo":[1,2,3]}}"#;
        let parsed: SidecarResponse = serde_json::from_str(raw).expect("parse");
        match parsed {
            SidecarResponse::Event {
                session_id,
                sdk_event,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(sdk_event["foo"], serde_json::json!([1, 2, 3]));
            }
            other => panic!("expected Event, got {other:?}"),
        }
    }

    #[test]
    fn session_error_response_deserialises() {
        let raw = r#"{"type":"session_error","session_id":"s","error":"boom"}"#;
        let parsed: SidecarResponse = serde_json::from_str(raw).expect("parse");
        match parsed {
            SidecarResponse::SessionError { session_id, error } => {
                assert_eq!(session_id, "s");
                assert_eq!(error, "boom");
            }
            other => panic!("expected SessionError, got {other:?}"),
        }
    }
}
