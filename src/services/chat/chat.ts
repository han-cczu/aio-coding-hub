// Usage: M0 chat service layer. Wraps the chat sidecar Tauri commands with
// raw `invoke()` calls. Specta bindings for these commands are regenerated
// during the integration phase; until then this module owns the contract.

import { invoke } from "@tauri-apps/api/core";

/**
 * Subset of an SDK message forwarded over the `chat-event-{session_id}`
 * channel. The sidecar relays the raw Claude Agent SDK event JSON, so the
 * shape is intentionally permissive: only the fields we read are typed,
 * every other field is opaque.
 *
 * Two delivery shapes matter for rendering text:
 *   - `type:"assistant"` carries a complete `message` snapshot for the turn
 *     (`message.content[]` with text blocks) — the authoritative full text.
 *   - `type:"stream_event"` carries incremental partials in `event`; we read
 *     `content_block_delta` / `text_delta` tokens from there for live typing.
 */
export type ChatSdkEvent = {
  type?: string;
  message?: ChatSdkAssistantMessage;
  /** Present on `type:"stream_event"` partials (Anthropic streaming envelope). */
  event?: ChatSdkStreamEvent;
  [key: string]: unknown;
};

/**
 * Inner Anthropic streaming event carried by a `type:"stream_event"` envelope.
 * Mirrors the SSE message shape (`content_block_delta`, `message_start`, …).
 * Only the `content_block_delta` text path is typed; the rest is opaque.
 */
export type ChatSdkStreamEvent = {
  type?: string;
  index?: number;
  delta?: ChatSdkStreamDelta;
  [key: string]: unknown;
};

export type ChatSdkStreamDelta = {
  type?: string;
  text?: string;
  [key: string]: unknown;
};

export type ChatSdkAssistantMessage = {
  role?: string;
  content?: ChatSdkContentBlock[];
  [key: string]: unknown;
};

/**
 * Claude SDK content block. M0 only renders `text` deltas; the other block
 * types are forwarded so M1 features (tool use, thinking) can opt in later.
 */
export type ChatSdkContentBlock = {
  type?: string;
  text?: string;
  [key: string]: unknown;
};

export type ChatErrorPayload = {
  error: string;
};

/**
 * Claude-native permission modes, forwarded verbatim to the `claude`
 * subprocess (`--permission-mode <mode>`). Values match the Claude Agent
 * SDK `PermissionMode` union exactly — do NOT translate to snake_case:
 *   - `plan`             — Claude plans first; no tool execution until approved.
 *   - `acceptEdits`      — auto-accepts file edits.
 *   - `default`          — honours settings.json allow/ask/deny rules.
 *   - `auto`             — the model decides whether to prompt.
 *   - `dontAsk`          — allowed tools run, everything else is denied (no prompt).
 *   - `bypassPermissions` — auto-approves every tool use (dangerous).
 */
export type ChatPermissionMode =
  | "plan"
  | "acceptEdits"
  | "default"
  | "auto"
  | "dontAsk"
  | "bypassPermissions";

/**
 * Launcher used to spawn the session's `claude` process. These are the only
 * two values the backend `normalize_launcher` accepts verbatim:
 *   - `reclaude` — force the user's launcher (runs an auth/config sync, then
 *     delegates to `claude`).
 *   - `claude`   — connect directly, skipping that sync.
 * "Auto" is expressed by OMITTING the field entirely, which lets the backend
 * pick (env override → `reclaude` → `claude`). Do not send `"auto"` as a value.
 */
export type ChatLauncher = "reclaude" | "claude";

/**
 * Options for `chat_create_session`. The permission mode, launcher, model, and
 * optional tool allow/deny lists are fixed for the lifetime of the session in
 * M0; changing them mid-session is a later milestone.
 */
export type ChatCreateSessionOptions = {
  cwd: string;
  permissionMode?: ChatPermissionMode;
  launcher?: ChatLauncher;
  /**
   * Model passed verbatim to `claude --model`. Accepts an alias (`opus` /
   * `sonnet` / `haiku`) or a full name (e.g. `claude-opus-4-8`). OMIT to use
   * claude's own default — do not send an empty string.
   */
  model?: string;
  allowedTools?: string[];
  disallowedTools?: string[];
};

export function chatEventName(sessionId: string): string {
  return `chat-event-${sessionId}`;
}

export function chatErrorEventName(sessionId: string): string {
  return `chat-error-${sessionId}`;
}

// IPC arg convention (matches Specta-generated AIO commands):
//   Rust:  `pub async fn xxx(input: XxxInput)` where `XxxInput` is
//          `#[serde(rename_all = "camelCase")]`.
//   JS:    invoke("xxx", { input: { camelCaseField: ... } })
// See e.g. `bindings.ts` for `mcpServerDelete`:
//   TAURI_INVOKE("mcp_server_delete", { input })
//
// Once Specta bindings are regenerated (CI / non-Windows host), these
// callers can migrate to `commands.chat*` for full type safety.

export async function chatDefaultCwd(): Promise<string> {
  return invoke<string>("chat_default_cwd");
}

export async function chatCreateSession(opts: ChatCreateSessionOptions): Promise<string> {
  const { cwd, permissionMode, launcher, model, allowedTools, disallowedTools } = opts;
  // Only forward optional keys when set, so the backend sees a clean input
  // (and so an omitted launcher/model stays absent → backend default, rather
  // than null/empty).
  const input: Record<string, unknown> = { cwd };
  if (permissionMode) input.permissionMode = permissionMode;
  if (launcher) input.launcher = launcher;
  if (model && model.trim().length > 0) input.model = model.trim();
  if (allowedTools && allowedTools.length > 0) input.allowedTools = allowedTools;
  if (disallowedTools && disallowedTools.length > 0) input.disallowedTools = disallowedTools;
  return invoke<string>("chat_create_session", { input });
}

export async function chatSendMessage(sessionId: string, content: string): Promise<void> {
  await invoke<void>("chat_send_message", { input: { sessionId, content } });
}

export async function chatCloseSession(sessionId: string): Promise<void> {
  await invoke<void>("chat_close_session", { input: { sessionId } });
}
