// Usage: M0 chat store. Tracks the single in-flight session: the chronological
// list of user/assistant messages and the partial assistant text streamed in
// from `chat-event-{session_id}`. Subscribers connect via `useChatStore`
// (useSyncExternalStore) just like `services/gateway/traceStore.ts`.

import { useSyncExternalStore } from "react";
import { emitListenerSnapshot } from "../utils/listeners";
import type {
  ChatPermissionMode,
  ChatSdkContentBlock,
  ChatSdkEvent,
  ChatSlashCommand,
} from "../services/chat/chat";

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

/**
 * Model preset selected in the UI:
 *   - `default` — UI-only sentinel; OMIT `--model` so claude uses its default.
 *   - `opus` / `sonnet` / `haiku` — passed verbatim as a `claude --model` alias.
 *   - `custom` — use the free-text `modelCustom` full name instead.
 */
export type ChatModelPreset = "default" | "opus" | "sonnet" | "haiku" | "custom";

/** Default model preset — let claude pick. */
export const DEFAULT_CHAT_MODEL_PRESET: ChatModelPreset = "default";

/**
 * Resolve the UI model selection to the value sent to `claude --model`, or
 * `undefined` to omit the flag entirely. `custom` with a blank box falls back
 * to omit (claude default), mirroring how the backend treats blank as unset.
 */
export function resolveChatModel(preset: ChatModelPreset, custom: string): string | undefined {
  if (preset === "default") return undefined;
  if (preset === "custom") {
    const trimmed = custom.trim();
    return trimmed.length > 0 ? trimmed : undefined;
  }
  return preset;
}

/**
 * Merge the two slash-command sources into one deduped, ordered list for the
 * autocomplete palette:
 *   - `backend` — best-effort list from `chat_list_slash_commands` (rich:
 *     carries description + source; available before the session starts).
 *   - `sessionNames` — bare command names from the session's `system/init`
 *     event (authoritative once connected; may include commands the backend
 *     could not scan, e.g. MCP commands).
 *
 * Dedup is by `name`, first occurrence wins. Backend entries come first (so
 * their description/source metadata is preferred and the curated ordering —
 * builtin, skill, command — is kept); session-only names are appended with a
 * neutral `builtin` badge since we have no metadata for them.
 */
export function mergeSlashCommands(
  backend: ChatSlashCommand[],
  sessionNames: string[]
): ChatSlashCommand[] {
  const byName = new Map<string, ChatSlashCommand>();
  for (const cmd of backend) {
    if (cmd && typeof cmd.name === "string" && cmd.name && !byName.has(cmd.name)) {
      byName.set(cmd.name, cmd);
    }
  }
  for (const raw of sessionNames) {
    // Tolerate a stray leading slash from the session payload just in case.
    const name = typeof raw === "string" ? raw.replace(/^\/+/, "") : "";
    if (name && !byName.has(name)) {
      byName.set(name, { name, source: "builtin" });
    }
  }
  return [...byName.values()];
}

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
  /**
   * Model preset chosen for this chat. Same lifetime rule as `permissionMode`.
   * `custom` defers to `modelCustom`.
   */
  model: ChatModelPreset;
  /** Free-text full model name, used only when `model === "custom"`. */
  modelCustom: string;
  /**
   * Best-effort slash commands from `chat_list_slash_commands` (rich metadata,
   * available before the session starts). Combine with `slashCommandNames` via
   * {@link mergeSlashCommands} for the palette.
   */
  slashCommandsBackend: ChatSlashCommand[];
  /**
   * Authoritative slash command names from the session's `system/init` event
   * (bare names). Empty until the session emits init.
   */
  slashCommandNames: string[];
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
  model: DEFAULT_CHAT_MODEL_PRESET,
  modelCustom: "",
  slashCommandsBackend: [],
  slashCommandNames: [],
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
 * Update the model preset for the (not-yet-created) session. Same gating
 * contract as {@link setChatPermissionMode}.
 */
export function setChatModel(model: ChatModelPreset) {
  if (state.model === model) return;
  setState({ ...state, model });
}

/** Update the free-text custom model name (only used when `model === "custom"`). */
export function setChatModelCustom(modelCustom: string) {
  if (state.modelCustom === modelCustom) return;
  setState({ ...state, modelCustom });
}

/**
 * Store the best-effort slash command list fetched from the backend for the
 * current cwd. Replaces any previous list (re-fetched when cwd changes).
 */
export function setChatSlashCommandsBackend(commands: ChatSlashCommand[]) {
  setState({ ...state, slashCommandsBackend: commands });
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

function arraysEqual(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) {
    if (a[i] !== b[i]) return false;
  }
  return true;
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
 *   - `system` / `init`: capture the authoritative `slash_commands` list for
 *     the `/` autocomplete palette.
 *   - `result` / `result_end`: end the turn (`streaming=false`, `sending` off).
 *
 * Everything else (system status, rate_limit_event, tool_use snapshots,
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

  if (type === "system" && event.subtype === "init") {
    // Authoritative slash command set for this session. Only update when it
    // actually changes to avoid spurious re-renders on repeated init events.
    const names = Array.isArray(event.slash_commands)
      ? event.slash_commands.filter((n): n is string => typeof n === "string")
      : [];
    if (names.length > 0 && !arraysEqual(names, state.slashCommandNames)) {
      setState({ ...state, slashCommandNames: names });
    }
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
