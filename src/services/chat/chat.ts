// Usage: M0 chat service layer. Wraps the chat sidecar Tauri commands with
// raw `invoke()` calls. Specta bindings for these commands are regenerated
// during the integration phase; until then this module owns the contract.

import { invoke } from "@tauri-apps/api/core";

/**
 * Subset of an SDK message forwarded over the `chat-event-{session_id}`
 * channel. The sidecar relays the raw Claude Agent SDK event JSON, so the
 * shape is intentionally permissive: only the discriminator and message
 * payload are typed, every other field is opaque.
 */
export type ChatSdkEvent = {
  type?: string;
  message?: ChatSdkAssistantMessage;
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

export async function chatCreateSession(cwd: string): Promise<string> {
  return invoke<string>("chat_create_session", { input: { cwd } });
}

export async function chatSendMessage(sessionId: string, content: string): Promise<void> {
  await invoke<void>("chat_send_message", { input: { sessionId, content } });
}

export async function chatCloseSession(sessionId: string): Promise<void> {
  await invoke<void>("chat_close_session", { input: { sessionId } });
}
