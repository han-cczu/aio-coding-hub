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

  it("accumulates assistant text from streaming SDK events", async () => {
    const { useChatStore, appendUserMessage, ingestChatEvent, resetChatStore } =
      await importFreshChatStore();
    resetChatStore();

    const { result } = renderHook(() => useChatStore());

    act(() => {
      appendUserMessage("hi");
    });

    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "Hello " }] },
      });
    });
    act(() => {
      ingestChatEvent({
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "world" }] },
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
