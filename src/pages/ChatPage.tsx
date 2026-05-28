// Usage: M0 chat page. One session per page load — no session tabs, no cwd
// picker, no tool / permission UX. Calls `chat_create_session` on first
// send, streams assistant text via `useChatEventStream`, and closes the
// session on unmount.

import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { useChatEventStream } from "../hooks/useChatEventStream";
import { chatCloseSession, chatCreateSession, chatSendMessage } from "../services/chat/chat";
import { logToConsole } from "../services/consoleLog";
import {
  appendUserMessage,
  resetChatStore,
  setChatError,
  setChatSessionId,
  setChatSessionPending,
  useChatStore,
  type ChatMessage,
} from "../stores/chatStore";
import { Button } from "../ui/Button";
import { Card } from "../ui/Card";
import { PageHeader } from "../ui/PageHeader";
import { Textarea } from "../ui/Textarea";
import { cn } from "../utils/cn";

/**
 * M0 hard-codes a default cwd. The cwd picker / per-session selection is
 * scheduled for M1. This intentionally avoids any FS probe so the page
 * stays usable even when the sidecar can't bind to the configured path.
 */
const DEFAULT_CWD = ".";

function formatErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function MessageBubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === "user";
  return (
    <div className={cn("flex w-full", isUser ? "justify-end" : "justify-start")}>
      <Card
        padding="sm"
        variant={isUser ? "raised" : "panel"}
        className={cn(
          "max-w-[75%] whitespace-pre-wrap break-words text-sm",
          isUser ? "bg-primary/10 text-foreground" : "text-foreground"
        )}
      >
        <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          {isUser ? "你" : "Assistant"}
        </div>
        <div className="mt-1 leading-relaxed">
          {message.text || (message.streaming ? "正在生成…" : "")}
          {message.streaming && message.text ? (
            <span
              aria-hidden="true"
              className="ml-0.5 inline-block h-3 w-[2px] translate-y-[1px] animate-pulse bg-foreground/60"
            />
          ) : null}
        </div>
      </Card>
    </div>
  );
}

export function ChatPage() {
  const { sessionId, messages, sessionPending, sending, error } = useChatStore();
  const [input, setInput] = useState("");
  // Track the latest sessionId via a ref so the unmount cleanup picks up
  // sessions created mid-lifetime without re-running on every change.
  const activeSessionIdRef = useRef<string | null>(sessionId);
  useEffect(() => {
    activeSessionIdRef.current = sessionId;
  }, [sessionId]);

  const messagesRef = useRef<HTMLDivElement | null>(null);

  useChatEventStream(sessionId);

  useEffect(() => {
    resetChatStore();
    return () => {
      const closingId = activeSessionIdRef.current;
      if (closingId) {
        chatCloseSession(closingId).catch((error) => {
          logToConsole(
            "warn",
            "关闭 chat 会话失败",
            { session_id: closingId, error: String(error) },
            "chat:page"
          );
        });
      }
      resetChatStore();
    };
  }, []);

  // Stick to the bottom when new content arrives. We intentionally avoid
  // virtualizing in M0 to keep the diff tiny.
  useEffect(() => {
    const node = messagesRef.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
  }, [messages]);

  const ensureSession = useCallback(async (): Promise<string> => {
    if (activeSessionIdRef.current) return activeSessionIdRef.current;
    setChatSessionPending(true);
    try {
      const newSessionId = await chatCreateSession(DEFAULT_CWD);
      setChatSessionId(newSessionId);
      activeSessionIdRef.current = newSessionId;
      return newSessionId;
    } catch (error) {
      setChatSessionPending(false);
      const message = formatErrorMessage(error);
      setChatError(message);
      throw error;
    }
  }, []);

  const handleSend = useCallback(async () => {
    const trimmed = input.trim();
    if (!trimmed || sending || sessionPending) return;

    setInput("");
    appendUserMessage(trimmed);

    try {
      const activeSessionId = await ensureSession();
      await chatSendMessage(activeSessionId, trimmed);
    } catch (error) {
      const message = formatErrorMessage(error);
      setChatError(message);
      toast("发送失败，请查看控制台日志");
      logToConsole("error", "chat 消息发送失败", { error: message }, "chat:page");
    }
  }, [ensureSession, input, sending, sessionPending]);

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
        event.preventDefault();
        handleSend();
      }
    },
    [handleSend]
  );

  const sendDisabled = !input.trim() || sending || sessionPending;
  const statusLabel = sessionPending
    ? "会话启动中…"
    : sending
      ? "等待回复…"
      : sessionId
        ? "已连接"
        : "未连接";

  return (
    <div className="flex h-full flex-col gap-4 overflow-hidden">
      <PageHeader title="Chat" subtitle={`cwd: ${DEFAULT_CWD} · ${statusLabel}`} />

      <Card padding="none" className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div
          ref={messagesRef}
          className="scrollbar-overlay flex-1 space-y-3 overflow-y-auto px-4 py-4"
        >
          {messages.length === 0 ? (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              发送一条消息开启对话
            </div>
          ) : (
            messages.map((message) => <MessageBubble key={message.id} message={message} />)
          )}
        </div>

        {error ? (
          <div className="border-t border-rose-200 bg-rose-50 px-4 py-2 text-xs text-rose-700 dark:border-rose-700 dark:bg-rose-900/30 dark:text-rose-300">
            {error}
          </div>
        ) : null}

        <div className="flex items-end gap-2 border-t border-line bg-surface-panel px-4 py-3">
          <Textarea
            value={input}
            onChange={(event) => setInput(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="输入消息，回车发送，Shift+回车换行"
            rows={2}
            className="min-h-[44px]"
            disabled={sessionPending}
          />
          <div className="flex flex-col gap-2">
            <Button variant="primary" size="md" onClick={handleSend} disabled={sendDisabled}>
              发送
            </Button>
            {/* M0: cancel is not wired to a real cancellation yet. Disabled
                placeholder so the layout matches the eventual M1 control. */}
            <Button variant="secondary" size="md" disabled aria-label="取消（M1 实现）">
              取消
            </Button>
          </div>
        </div>
      </Card>
    </div>
  );
}
