/**
 * chat-bridge entry point.
 *
 * Reads NDJSON from stdin, dispatches to SessionManager, writes NDJSON to
 * stdout. One JSON object per line, no embedded newlines. The first line we
 * emit is always the `ready` event so the host knows the sidecar is alive
 * and can read the SDK version.
 */

import { createInterface } from 'node:readline';
import { SessionManager } from './session.js';
import {
  isInboundMessage,
  type InboundMessage,
  type OutboundMessage,
} from './protocol.js';

// Both constants are inlined at build time by scripts/build.mjs via the
// esbuild `define` option. The fallback values cover `tsc --noEmit` /
// running unbundled under ts-node, where the substitution does not occur.
declare const __SIDECAR_VERSION__: string;
declare const __SDK_VERSION__: string;

const SIDECAR_VERSION =
  typeof __SIDECAR_VERSION__ !== 'undefined' ? __SIDECAR_VERSION__ : 'dev';
const SDK_VERSION =
  typeof __SDK_VERSION__ !== 'undefined' ? __SDK_VERSION__ : 'unknown';

function emit(msg: OutboundMessage): void {
  // One JSON object per line. Use process.stdout.write to keep control over
  // buffering; console.log would also work but inserts platform-specific
  // line endings on Windows.
  process.stdout.write(JSON.stringify(msg) + '\n');
}

function main(): void {
  const manager = new SessionManager(
    (sessionId, sdkEvent) => emit({ type: 'event', session_id: sessionId, sdk_event: sdkEvent }),
    (sessionId, error) => emit({ type: 'session_error', session_id: sessionId, error }),
  );

  // Announce readiness immediately so the host can proceed without waiting
  // for the first inbound message.
  emit({ type: 'ready', sidecar_version: SIDECAR_VERSION, sdk_version: SDK_VERSION });

  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });

  rl.on('line', (line) => {
    const trimmed = line.trim();
    if (trimmed.length === 0) return;

    let parsed: unknown;
    try {
      parsed = JSON.parse(trimmed);
    } catch (err) {
      // No session context for a parse failure — log to stderr so the host
      // can surface it but do not crash the pipeline.
      process.stderr.write(`chat-bridge: invalid JSON line: ${formatError(err)}\n`);
      return;
    }

    if (!isInboundMessage(parsed)) {
      process.stderr.write(`chat-bridge: unknown inbound message: ${trimmed}\n`);
      return;
    }

    void dispatch(manager, parsed);
  });

  rl.on('close', () => {
    // stdin closed by host -> shut everything down cleanly.
    void manager.closeAll().finally(() => process.exit(0));
  });

  // Convert uncaught errors into stderr lines so they are visible to the
  // host without taking down the process silently.
  process.on('uncaughtException', (err) => {
    process.stderr.write(`chat-bridge: uncaughtException: ${formatError(err)}\n`);
  });
  process.on('unhandledRejection', (err) => {
    process.stderr.write(`chat-bridge: unhandledRejection: ${formatError(err)}\n`);
  });
}

async function dispatch(manager: SessionManager, msg: InboundMessage): Promise<void> {
  switch (msg.type) {
    case 'ping':
      emit({ type: 'pong' });
      return;

    case 'create_session':
      try {
        await manager.create(msg.session_id, msg.cwd);
        emit({ type: 'session_ready', session_id: msg.session_id });
      } catch (err) {
        emit({ type: 'session_error', session_id: msg.session_id, error: formatError(err) });
      }
      return;

    case 'send_message':
      try {
        manager.send(msg.session_id, msg.content);
      } catch (err) {
        emit({ type: 'session_error', session_id: msg.session_id, error: formatError(err) });
      }
      return;

    case 'close_session':
      try {
        await manager.close(msg.session_id);
      } catch (err) {
        emit({ type: 'session_error', session_id: msg.session_id, error: formatError(err) });
      }
      return;
  }
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.stack ?? err.message ?? err.name;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

main();
