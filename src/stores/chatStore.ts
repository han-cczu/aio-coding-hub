// Usage: M0 chat store. Tracks the single in-flight session: the chronological
// list of user/assistant messages and the partial assistant text streamed in
// from `chat-event-{session_id}`. Subscribers connect via `useChatStore`
// (useSyncExternalStore) just like `services/gateway/traceStore.ts`.

import { useSyncExternalStore } from "react";
import { emitListenerSnapshot } from "../utils/listeners";
import type { ChatSdkContentBlock, ChatSdkEvent } from "../services/chat/chat";

export type ChatRole = "user" | "assistant";

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
};

type Listener = () => void;

type ChatStoreState = ChatStoreSnapshot;

const INITIAL_STATE: ChatStoreState = {
  sessionId: null,
  messages: [],
  sessionPending: false,
  sending: false,
  error: null,
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
 * M0 surfaces only assistant text. Other event types (system, user, result,
 * tool_use, …) are recognised enough to flip `sending` off when the turn
 * ends; the rest is intentionally ignored until M1.
 */
export function ingestChatEvent(event: ChatSdkEvent | null | undefined) {
  if (!event || typeof event !== "object") return;
  const type = typeof event.type === "string" ? event.type : "";

  if (type === "assistant") {
    const text = extractTextFromBlocks(event.message?.content);
    if (!text) return;
    setState({
      ...state,
      messages: appendTextToAssistant(state.messages, text),
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
