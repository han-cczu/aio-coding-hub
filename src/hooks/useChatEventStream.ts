// Usage: Bridge Tauri chat events into the in-memory chat store. Subscribes
// to `chat-event-{session_id}` and `chat-error-{session_id}` for the active
// session and tears the listeners down on session change / unmount.

import { useEffect } from "react";
import { listenDesktopEvent } from "../services/desktop/event";
import {
  chatErrorEventName,
  chatEventName,
  type ChatErrorPayload,
  type ChatSdkEvent,
} from "../services/chat/chat";
import { logToConsole } from "../services/consoleLog";
import { ingestChatError, ingestChatEvent } from "../stores/chatStore";

function logSubscribeFailure(stage: string, sessionId: string, error: unknown) {
  logToConsole(
    "warn",
    "chat 事件订阅失败",
    { stage, session_id: sessionId, error: String(error) },
    "chat:event_stream"
  );
}

export function useChatEventStream(sessionId: string | null) {
  useEffect(() => {
    if (!sessionId) return;

    let cancelled = false;
    let cleanupEvent: (() => void) | null = null;
    let cleanupError: (() => void) | null = null;

    const eventName = chatEventName(sessionId);
    const errorName = chatErrorEventName(sessionId);

    listenDesktopEvent<ChatSdkEvent>(eventName, (payload) => {
      ingestChatEvent(payload);
    })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }
        cleanupEvent = unlisten;
      })
      .catch((error) => logSubscribeFailure("chat-event", sessionId, error));

    listenDesktopEvent<ChatErrorPayload>(errorName, (payload) => {
      const message = typeof payload?.error === "string" ? payload.error : "未知错误";
      ingestChatError(message);
    })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }
        cleanupError = unlisten;
      })
      .catch((error) => logSubscribeFailure("chat-error", sessionId, error));

    return () => {
      cancelled = true;
      cleanupEvent?.();
      cleanupError?.();
    };
  }, [sessionId]);
}
