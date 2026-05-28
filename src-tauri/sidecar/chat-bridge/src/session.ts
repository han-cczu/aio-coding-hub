/**
 * Session lifecycle wrapper around the Claude Agent SDK.
 *
 * NOTE: The current @anthropic-ai/claude-agent-sdk (0.3.x) TypeScript surface
 * does NOT expose a `ClaudeSDKClient` with explicit `connect/query/
 * receiveMessages/disconnect` methods. Instead it exports a `query()`
 * function that returns a `Query` AsyncGenerator and consumes the prompt
 * as either a string or `AsyncIterable<SDKUserMessage>` (streaming input
 * mode). To honour the M0 protocol's per-session multi-turn semantics, we
 * wrap each session with:
 *
 *   1. a `PushableUserMessages` async iterable we drive from `send()`, and
 *   2. a background pump that consumes the resulting `Query` and forwards
 *      every SDK message back to the host through `onEvent`.
 *
 * `create()` resolves once the SDK reports `system.init` (or any first
 * SDK message), which we treat as the analogue of `await connect()`.
 */

import { query, type Query, type SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';

type EventHandler = (sessionId: string, sdkEvent: unknown) => void;
type ErrorHandler = (sessionId: string, error: string) => void;

interface PushableUserMessages extends AsyncIterable<SDKUserMessage> {
  push(content: string): void;
  end(): void;
}

function createPushable(sessionId: string): PushableUserMessages {
  const queue: SDKUserMessage[] = [];
  const waiters: Array<(value: IteratorResult<SDKUserMessage>) => void> = [];
  let finished = false;

  return {
    push(content: string) {
      if (finished) return;
      const msg: SDKUserMessage = {
        type: 'user',
        session_id: sessionId,
        message: { role: 'user', content },
        parent_tool_use_id: null,
      };
      const waiter = waiters.shift();
      if (waiter) {
        waiter({ value: msg, done: false });
      } else {
        queue.push(msg);
      }
    },
    end() {
      if (finished) return;
      finished = true;
      while (waiters.length > 0) {
        const waiter = waiters.shift();
        if (waiter) waiter({ value: undefined as unknown as SDKUserMessage, done: true });
      }
    },
    [Symbol.asyncIterator]() {
      return {
        next(): Promise<IteratorResult<SDKUserMessage>> {
          if (queue.length > 0) {
            return Promise.resolve({ value: queue.shift()!, done: false });
          }
          if (finished) {
            return Promise.resolve({ value: undefined as unknown as SDKUserMessage, done: true });
          }
          return new Promise((resolve) => {
            waiters.push(resolve);
          });
        },
        return(): Promise<IteratorResult<SDKUserMessage>> {
          finished = true;
          while (waiters.length > 0) {
            const waiter = waiters.shift();
            if (waiter) waiter({ value: undefined as unknown as SDKUserMessage, done: true });
          }
          return Promise.resolve({ value: undefined as unknown as SDKUserMessage, done: true });
        },
      };
    },
  };
}

interface Session {
  id: string;
  inputs: PushableUserMessages;
  q: Query;
  pump: Promise<void>;
}

export class SessionManager {
  private readonly sessions = new Map<string, Session>();

  constructor(
    private readonly onEvent: EventHandler,
    private readonly onError: ErrorHandler,
  ) {}

  /**
   * Create a new session bound to `cwd`. Spawns the SDK Query, starts a
   * background pump, and resolves once the SDK is ready to accept input.
   *
   * For M0 we resolve eagerly after wiring up the pump (no need to await
   * the first SDK message — the protocol's `session_ready` ack is emitted
   * by the caller). The actual SDK init happens lazily when the iterator
   * is consumed.
   */
  async create(sessionId: string, cwd: string): Promise<void> {
    if (this.sessions.has(sessionId)) {
      throw new Error(`session ${sessionId} already exists`);
    }

    const inputs = createPushable(sessionId);
    // Resolve the Claude Code CLI binary from the env var the Rust host sets
    // (AIO_CHAT_CLAUDE_CODE_PATH). The SDK normally ships a platform-specific
    // optional dep that supplies this binary, but pnpm/Windows installs
    // sometimes skip optional deps, so we explicitly point at the system-wide
    // `claude` that AIO already manages via cli_manager. Falls back to SDK
    // defaults when the env var is unset.
    const claudeCodePath = process.env.AIO_CHAT_CLAUDE_CODE_PATH;
    const q = query({
      prompt: inputs,
      options: {
        cwd,
        ...(claudeCodePath ? { pathToClaudeCodeExecutable: claudeCodePath } : {}),
        // M0 does not implement canUseTool/permission; rely on SDK defaults.
      },
    });

    const session: Session = {
      id: sessionId,
      inputs,
      q,
      pump: (async () => {
        try {
          for await (const sdkEvent of q) {
            this.onEvent(sessionId, sdkEvent);
          }
        } catch (err) {
          this.onError(sessionId, formatError(err));
        }
      })(),
    };

    this.sessions.set(sessionId, session);
  }

  /**
   * Push a user message into an existing session. The SDK consumes the
   * pushed message, runs its agent loop, and emits resulting SDKMessages
   * through the background pump. No backpressure is applied: each push
   * is non-blocking.
   */
  send(sessionId: string, content: string): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      throw new Error(`session ${sessionId} not found`);
    }
    session.inputs.push(content);
  }

  /**
   * Close a session: signal end-of-input, ask the SDK to terminate its
   * subprocess, and await the pump so we never leak background work.
   */
  async close(sessionId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      // Treat duplicate/unknown close as a no-op so the host can be sloppy.
      return;
    }
    this.sessions.delete(sessionId);

    session.inputs.end();
    try {
      session.q.close();
    } catch {
      // ignore — close() on already-finished Query may throw on some SDKs
    }
    try {
      await session.pump;
    } catch {
      // pump errors already reported via onError
    }
  }

  async closeAll(): Promise<void> {
    const ids = Array.from(this.sessions.keys());
    await Promise.all(ids.map((id) => this.close(id)));
  }

  has(sessionId: string): boolean {
    return this.sessions.has(sessionId);
  }
}

function formatError(err: unknown): string {
  if (err instanceof Error) {
    return err.message || err.name || 'unknown error';
  }
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
