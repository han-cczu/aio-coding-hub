// Usage: M0 chat store. Tracks the single in-flight session: the chronological
// list of user/assistant messages and the partial assistant text streamed in
// from `chat-event-{session_id}`. Subscribers connect via `useChatStore`
// (useSyncExternalStore) just like `services/gateway/traceStore.ts`.

import { useSyncExternalStore } from "react";
import { emitListenerSnapshot } from "../utils/listeners";
import type { ChatPermissionMode, ChatSdkContentBlock, ChatSdkEvent } from "../services/chat/chat";

export type ChatRole = "user" | "assistant";

/**
 * Default permission mode for a fresh chat. `default` honours the user's
 * settings.json allow/ask/deny rules — the safest baseline.
 */
export const DEFAULT_CHAT_PERMISSION_MODE: ChatPermissionMode = "default";

/**
 * Launcher choice as held in the UI. `"auto"` is a UI-only sentinel — when the
 * session is created it maps to OMITTING the launcher field so the backend
 * auto-selects (`reclaude` preferred, falling back to `claude`). The explicit
 * `"reclaude"` / `"claude"` values map straight through to the IPC `launcher`.
 */
export type ChatLauncherChoice = "auto" | "reclaude" | "claude";

/** Default launcher choice — let the backend auto-select. */
export const DEFAULT_CHAT_LAUNCHER: ChatLauncherChoice = "auto";

export type ChatMessage = {
  /** Local monotonically increasing id; not the SDK message id. */
  id: string;
  role: ChatRole;
  /** Concatenated text content. For assistant messages this grows as deltas arrive. */
  text: string;
  /** True while the assistant message is still streaming. */
  streaming: boolean;
  /** Wall-clock timestamp the message first appeared in the store. */
  createdAtMs: number;
};

export type ChatStoreSnapshot = {
  sessionId: string | null;
  messages: ChatMessage[];
  /** True between createSession start and the first session_ready / error. */
  sessionPending: boolean;
  /** True between sendMessage call and result/result_end SDK event. */
  sending: boolean;
  /** Last user-visible error (sticky until cleared / next send). */
  error: string | null;
  /**
   * Claude-native permission mode chosen for this chat. Editable only until
   * the session is created; afterwards the selector is locked (M0 fixes the
   * mode for the session lifetime).
   */
  permissionMode: ChatPermissionMode;
  /**
   * Launcher chosen for this chat (`auto` / `reclaude` / `claude`). Same
   * lifetime rule as `permissionMode`: editable until the session exists.
   */
  launcher: ChatLauncherChoice;
};

type Listener = () => void;

type ChatStoreState = ChatStoreSnapshot;

const INITIAL_STATE: ChatStoreState = {
  sessionId: null,
  messages: [],
  sessionPending: false,
  sending: false,
  error: null,
  permissionMode: DEFAULT_CHAT_PERMISSION_MODE,
  launcher: DEFAULT_CHAT_LAUNCHER,
};

let state: ChatStoreState = INITIAL_STATE;

const listeners = new Set<Listener>();

let messageSeq = 0;

function nextMessageId(): string {
  messageSeq += 1;
  return `m${messageSeq}`;
}

function emit() {
  emitListenerSnapshot(listeners, (listener) => listener());
}

function setState(next: ChatStoreState) {
  state = next;
  emit();
}

export function subscribeChatStore(listener: Listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getChatStoreSnapshot(): ChatStoreSnapshot {
  return state;
}

export function useChatStore(): ChatStoreSnapshot {
  return useSyncExternalStore(
    subscribeChatStore,
    () => state,
    () => state
  );
}

export function resetChatStore() {
  messageSeq = 0;
  setState(INITIAL_STATE);
}

export function setChatSessionPending(pending: boolean) {
  setState({ ...state, sessionPending: pending });
}

export function setChatSessionId(sessionId: string | null) {
  setState({ ...state, sessionId, sessionPending: false });
}

export function setChatError(error: string | null) {
  setState({ ...state, error, sending: false });
}

/**
 * Update the permission mode for the (not-yet-created) session. Callers
 * should gate this on `sessionId === null`; the store does not enforce it
 * so tests can drive it freely.
 */
export function setChatPermissionMode(mode: ChatPermissionMode) {
  if (state.permissionMode === mode) return;
  setState({ ...state, permissionMode: mode });
}

/**
 * Update the launcher choice for the (not-yet-created) session. Same gating
 * contract as {@link setChatPermissionMode}.
 */
export function setChatLauncher(launcher: ChatLauncherChoice) {
  if (state.launcher === launcher) return;
  setState({ ...state, launcher });
}

/**
 * Append a user message and mark the store as awaiting an assistant reply.
 * Returns the synthesized message id so callers can correlate locally.
 */
export function appendUserMessage(text: string): string {
  const id = nextMessageId();
  const message: ChatMessage = {
    id,
    role: "user",
    text,
    streaming: false,
    createdAtMs: Date.now(),
  };
  setState({
    ...state,
    messages: [...state.messages, message],
    sending: true,
    error: null,
  });
  return id;
}

function ensureStreamingAssistant(messages: ChatMessage[]): {
  messages: ChatMessage[];
  index: number;
} {
  const lastIndex = messages.length - 1;
  const last = lastIndex >= 0 ? messages[lastIndex] : null;
  if (last && last.role === "assistant" && last.streaming) {
    return { messages, index: lastIndex };
  }

  const fresh: ChatMessage = {
    id: nextMessageId(),
    role: "assistant",
    text: "",
    streaming: true,
    createdAtMs: Date.now(),
  };
  return { messages: [...messages, fresh], index: messages.length };
}

function appendTextToAssistant(messages: ChatMessage[], delta: string): ChatMessage[] {
  if (!delta) return messages;
  const ensured = ensureStreamingAssistant(messages);
  const target = ensured.messages[ensured.index];
  const updated: ChatMessage = {
    ...target,
    text: target.text + delta,
  };
  const nextMessages = ensured.messages.slice();
  nextMessages[ensured.index] = updated;
  return nextMessages;
}

function extractTextFromBlocks(blocks: ChatSdkContentBlock[] | undefined): string {
  if (!Array.isArray(blocks)) return "";
  let out = "";
  for (const block of blocks) {
    if (block && block.type === "text" && typeof block.text === "string") {
      out += block.text;
    }
  }
  return out;
}

/**
 * Pull a streaming text token out of a Claude `stream_event` partial.
 *
 * Field path is the empirically-captured shape (not inferred):
 *   { type:"stream_event", event:{ type:"content_block_delta",
 *       delta:{ type:"text_delta", text:"…" } } }
 *
 * Returns "" for any other stream_event (message_start, content_block_start,
 * input_json_delta for tool args, message_delta/stop, …) so they're ignored.
 */
function extractStreamTextDelta(event: ChatSdkEvent): string {
  if (event.type !== "stream_event") return "";
  const inner = event.event;
  if (!inner || inner.type !== "content_block_delta") return "";
  const delta = inner.delta;
  if (!delta || delta.type !== "text_delta") return "";
  return typeof delta.text === "string" ? delta.text : "";
}

/**
 * Replace (set, not append) the current streaming assistant message's text
 * with an authoritative full value — used when the turn's complete
 * `type:"assistant"` snapshot arrives, to correct any delta drift. Creates a
 * fresh streaming assistant message if none is in flight (e.g. a backend that
 * doesn't emit partial token events at all).
 */
function setTextOnStreamingAssistant(messages: ChatMessage[], fullText: string): ChatMessage[] {
  const ensured = ensureStreamingAssistant(messages);
  const target = ensured.messages[ensured.index];
  if (target.text === fullText) return ensured.messages;
  const nextMessages = ensured.messages.slice();
  nextMessages[ensured.index] = { ...target, text: fullText };
  return nextMessages;
}

function finalizeStreamingAssistant(messages: ChatMessage[]): ChatMessage[] {
  if (messages.length === 0) return messages;
  const lastIndex = messages.length - 1;
  const last = messages[lastIndex];
  if (last.role !== "assistant" || !last.streaming) return messages;
  const next = messages.slice();
  next[lastIndex] = { ...last, streaming: false };
  return next;
}

/**
 * Ingest a Claude Agent SDK event forwarded over `chat-event-{session_id}`.
 *
 * Text rendering follows the empirically-confirmed stream-json shapes:
 *   - `stream_event` → `content_block_delta` → `text_delta`: append the token
 *     to the in-flight assistant message for live typewriter output (the
 *     message is created on the first delta).
 *   - `assistant` (turn-complete snapshot): SET (replace) the assistant text
 *     with the authoritative full value, correcting any delta drift. We do
 *     NOT append here — appending after delta accumulation would double the
 *     text ("你好你好").
 *   - `result` / `result_end`: end the turn (`streaming=false`, `sending` off).
 *
 * Everything else (system init/status, rate_limit_event, tool_use snapshots,
 * message_start/stop partials, …) is intentionally ignored in M0.
 */
export function ingestChatEvent(event: ChatSdkEvent | null | undefined) {
  if (!event || typeof event !== "object") return;
  const type = typeof event.type === "string" ? event.type : "";

  if (type === "stream_event") {
    const delta = extractStreamTextDelta(event);
    if (!delta) return;
    setState({
      ...state,
      messages: appendTextToAssistant(state.messages, delta),
    });
    return;
  }

  if (type === "assistant") {
    const text = extractTextFromBlocks(event.message?.content);
    // Tool-use-only snapshots carry no text — skip them so they don't wipe
    // the streamed buffer with an empty string.
    if (!text) return;
    setState({
      ...state,
      messages: setTextOnStreamingAssistant(state.messages, text),
    });
    return;
  }

  if (type === "result" || type === "result_end") {
    setState({
      ...state,
      messages: finalizeStreamingAssistant(state.messages),
      sending: false,
    });
    return;
  }
}

export function ingestChatError(error: string) {
  setState({
    ...state,
    messages: finalizeStreamingAssistant(state.messages),
    sending: false,
    error,
  });
}
