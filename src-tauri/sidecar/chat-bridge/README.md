# chat-bridge

Node sidecar that bridges the AIO Coding Hub Rust gateway to
[`@anthropic-ai/claude-agent-sdk`](https://www.npmjs.com/package/@anthropic-ai/claude-agent-sdk).
The Rust host spawns it as a child process and talks NDJSON over stdio.

This is the **M0** implementation: session lifecycle and one-shot text
turn-taking only. No `canUseTool` / permission flow, no rich tool input
adapters — those land in M1.

---

## Build

```bash
pnpm install
pnpm run build
```

Build product: `dist/chat-bridge.js` (esbuild ESM bundle, `--target=node18`).

## Smoke test

```bash
# Linux / macOS
echo '{"type":"ping"}' | node dist/chat-bridge.js

# PowerShell
'{"type":"ping"}' | node dist/chat-bridge.js
```

Expected output (one JSON object per line):

```
{"type":"ready","sidecar_version":"0.1.0","sdk_version":"<sdk version>"}
{"type":"pong"}
```

The process exits as soon as stdin closes.

---

## Protocol (M0, frozen)

Transport: NDJSON over stdio. One JSON object per line, no embedded newlines.

### Rust -> Node (stdin)

```json
{"type":"ping"}
{"type":"create_session","session_id":"<uuid>","cwd":"<abs-path>"}
{"type":"send_message","session_id":"<uuid>","content":"<text>"}
{"type":"close_session","session_id":"<uuid>"}
```

### Node -> Rust (stdout)

```json
{"type":"ready","sidecar_version":"<x>","sdk_version":"<y>"}
{"type":"pong"}
{"type":"session_ready","session_id":"<uuid>"}
{"type":"event","session_id":"<uuid>","sdk_event":<SDK message JSON>}
{"type":"session_error","session_id":"<uuid>","error":"<msg>"}
```

The `ready` event is emitted unconditionally on startup, before any input
is consumed.

The `sdk_event` payload is the raw `SDKMessage` returned by the SDK; the
host is responsible for filtering / decoding (assistant text, tool calls,
result, etc.).

### Unknown / malformed lines

Lines that fail JSON.parse or do not match the four inbound message shapes
are logged to stderr and dropped. The sidecar never crashes on bad input.

---

## SDK API mapping note

The task spec referenced an older `ClaudeSDKClient` shape with
`connect / query / receiveMessages / disconnect`. The current TypeScript
SDK (`@anthropic-ai/claude-agent-sdk@0.3.x`) does **not** expose such a
class — its surface is a `query({prompt, options})` function returning a
`Query` AsyncGenerator that consumes either a string or an
`AsyncIterable<SDKUserMessage>` (streaming-input mode) as its prompt.

To preserve the M0 protocol's per-session multi-turn semantics on top of
the new API, each session owns a pushable async iterable that we feed into
`query()`. `send_message` pushes a `SDKUserMessage` into the iterable;
the background pump forwards every emitted `SDKMessage` as an `event`.
`close_session` ends the iterable and calls `Query.close()`.

If the protocol ever moves to wire-level fidelity with the SDK (per-turn
result aggregation, tool permission round-trips, etc.) this adapter is the
place to evolve.

---

## File layout

```
chat-bridge/
  package.json
  tsconfig.json
  README.md
  src/
    protocol.ts     # M0 message types + type guards
    session.ts      # SessionManager (sessionId -> Query pump)
    index.ts        # stdin readline -> dispatch -> stdout
  dist/
    chat-bridge.js  # built bundle (gitignored)
```
