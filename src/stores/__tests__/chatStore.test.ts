import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

async function importFreshChatStore() {
  vi.resetModules();
  return await import("../chatStore");
}

describe("stores/chatStore", () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it("appends user messages and flips into the sending state", async () => {
    const { useChatStore, appendUserMessage, resetChatStore } = await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());
    expect(result.current.messages).toEqual([]);
    expect(result.current.sending).toBe(false);

    act(() => {
      appendUserMessage("hello");
    });

    expect(result.current.messages).toHaveLength(1);
    expect(result.current.messages[0]).toMatchObject({
      role: "user",
      text: "hello",
      streaming: false,
    });
    expect(result.current.sending).toBe(true);
    expect(result.current.error).toBeNull();
  });

  it("treats each assistant snapshot as authoritative full text (set, not append)", async () => {
    const { useChatStore, appendUserMessage, ingestChatEvent, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      appendUserMessage("hi");
    });

    // Each `assistant` event carries the *complete* message snapshot for the
    // turn, so a later snapshot replaces (does not concatenate with) an
    // earlier one.
    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "Hello" }] },
      });
    });
    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "Hello world" }] },
      });
    });

    expect(result.current.messages).toHaveLength(2);
    const assistant = result.current.messages[1];
    expect(assistant.role).toBe("assistant");
    expect(assistant.text).toBe("Hello world");
    expect(assistant.streaming).toBe(true);
    expect(result.current.sending).toBe(true);

    act(() => {
      ingestChatEvent({ type: "result" });
    });

    expect(result.current.messages[1].streaming).toBe(false);
    expect(result.current.sending).toBe(false);
  });

  it("appends text_delta tokens from stream_event partials", async () => {
    const { useChatStore, appendUserMessage, ingestChatEvent, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      appendUserMessage("hi");
    });

    // Real captured partial shape: stream_event > content_block_delta > text_delta.
    act(() => {
      ingestChatEvent({
        type: "stream_event",
        event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "你" } },
      });
    });
    act(() => {
      ingestChatEvent({
        type: "stream_event",
        event: {
          type: "content_block_delta",
          index: 0,
          delta: { type: "text_delta", text: "好" },
        },
      });
    });

    expect(result.current.messages).toHaveLength(2);
    const assistant = result.current.messages[1];
    expect(assistant.role).toBe("assistant");
    expect(assistant.text).toBe("你好");
    expect(assistant.streaming).toBe(true);
  });

  it("does not double-count: delta tokens then a final assistant snapshot replaces", async () => {
    const { useChatStore, appendUserMessage, ingestChatEvent, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      appendUserMessage("hi");
    });

    // Stream the tokens "你好" live...
    act(() => {
      ingestChatEvent({
        type: "stream_event",
        event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "你" } },
      });
    });
    act(() => {
      ingestChatEvent({
        type: "stream_event",
        event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "好" } },
      });
    });
    expect(result.current.messages[1].text).toBe("你好");

    // ...then the turn-complete snapshot arrives with the same full text. It
    // must REPLACE, not append — otherwise we'd see "你好你好".
    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "你好" }] },
      });
    });

    expect(result.current.messages).toHaveLength(2);
    expect(result.current.messages[1].text).toBe("你好");

    act(() => {
      ingestChatEvent({ type: "result" });
    });
    expect(result.current.messages[1].streaming).toBe(false);
    expect(result.current.sending).toBe(false);
  });

  it("ignores non-text stream_event partials and other safe event types", async () => {
    const { useChatStore, ingestChatEvent, resetChatStore } = await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      // message_start / content_block_start envelopes carry no text_delta.
      ingestChatEvent({ type: "stream_event", event: { type: "message_start" } });
      ingestChatEvent({
        type: "stream_event",
        event: { type: "content_block_start", index: 0 },
      });
      // input_json_delta (tool args) is a delta but not a text_delta.
      ingestChatEvent({
        type: "stream_event",
        event: {
          type: "content_block_delta",
          index: 0,
          delta: { type: "input_json_delta", partial_json: "{" },
        },
      });
      // Unrelated top-level types.
      ingestChatEvent({ type: "system", subtype: "init" });
      ingestChatEvent({ type: "rate_limit_event" });
    });

    expect(result.current.messages).toEqual([]);
  });

  it("ignores assistant events with no text content blocks", async () => {
    const { useChatStore, ingestChatEvent, resetChatStore } = await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "tool_use", id: "x" }] },
      });
    });

    expect(result.current.messages).toEqual([]);
  });

  it("ingestChatError finalizes streaming and records the error", async () => {
    const { useChatStore, appendUserMessage, ingestChatEvent, ingestChatError, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      appendUserMessage("ping");
    });
    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "partial" }] },
      });
    });
    act(() => {
      ingestChatError("sidecar died");
    });

    expect(result.current.error).toBe("sidecar died");
    expect(result.current.sending).toBe(false);
    expect(result.current.messages[1].streaming).toBe(false);
    expect(result.current.messages[1].text).toBe("partial");
  });

  it("defaults the permission mode and updates it via setChatPermissionMode", async () => {
    const { useChatStore, setChatPermissionMode, resetChatStore, DEFAULT_CHAT_PERMISSION_MODE } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());
    expect(result.current.permissionMode).toBe(DEFAULT_CHAT_PERMISSION_MODE);
    expect(result.current.permissionMode).toBe("default");

    act(() => {
      setChatPermissionMode("plan");
    });
    expect(result.current.permissionMode).toBe("plan");

    act(() => {
      setChatPermissionMode("bypassPermissions");
    });
    expect(result.current.permissionMode).toBe("bypassPermissions");
  });

  it("defaults the launcher to auto and updates it via setChatLauncher", async () => {
    const { useChatStore, setChatLauncher, resetChatStore, DEFAULT_CHAT_LAUNCHER } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());
    expect(result.current.launcher).toBe(DEFAULT_CHAT_LAUNCHER);
    expect(result.current.launcher).toBe("auto");

    act(() => {
      setChatLauncher("reclaude");
    });
    expect(result.current.launcher).toBe("reclaude");

    act(() => {
      setChatLauncher("claude");
    });
    expect(result.current.launcher).toBe("claude");

    act(() => {
      setChatLauncher("auto");
    });
    expect(result.current.launcher).toBe("auto");
  });

  it("setChatSessionId clears the pending flag", async () => {
    const { useChatStore, setChatSessionPending, setChatSessionId, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      setChatSessionPending(true);
    });
    expect(result.current.sessionPending).toBe(true);

    act(() => {
      setChatSessionId("sess-1");
    });

    expect(result.current.sessionPending).toBe(false);
    expect(result.current.sessionId).toBe("sess-1");
  });
});
