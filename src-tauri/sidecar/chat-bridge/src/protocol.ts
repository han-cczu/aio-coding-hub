/**
 * M0 protocol (frozen) between the Rust gateway and the Node sidecar.
 *
 * Transport: NDJSON over stdio. One JSON object per line, no embedded newlines.
 *
 * Rust -> Node (stdin):
 *   {"type":"ping"}
 *   {"type":"create_session","session_id":"<uuid>","cwd":"<abs-path>"}
 *   {"type":"send_message","session_id":"<uuid>","content":"<text>"}
 *   {"type":"close_session","session_id":"<uuid>"}
 *
 * Node -> Rust (stdout):
 *   {"type":"ready","sidecar_version":"<x>","sdk_version":"<y>"}
 *   {"type":"pong"}
 *   {"type":"session_ready","session_id":"<uuid>"}
 *   {"type":"event","session_id":"<uuid>","sdk_event":<SDKMessage>}
 *   {"type":"session_error","session_id":"<uuid>","error":"<msg>"}
 */

// ----- Inbound (Rust -> Node) -----

export interface PingRequest {
  type: 'ping';
}

export interface CreateSessionRequest {
  type: 'create_session';
  session_id: string;
  cwd: string;
}

export interface SendMessageRequest {
  type: 'send_message';
  session_id: string;
  content: string;
}

export interface CloseSessionRequest {
  type: 'close_session';
  session_id: string;
}

export type InboundMessage =
  | PingRequest
  | CreateSessionRequest
  | SendMessageRequest
  | CloseSessionRequest;

// ----- Outbound (Node -> Rust) -----

export interface ReadyEvent {
  type: 'ready';
  sidecar_version: string;
  sdk_version: string;
}

export interface PongEvent {
  type: 'pong';
}

export interface SessionReadyEvent {
  type: 'session_ready';
  session_id: string;
}

export interface SdkEventForward {
  type: 'event';
  session_id: string;
  sdk_event: unknown;
}

export interface SessionErrorEvent {
  type: 'session_error';
  session_id: string;
  error: string;
}

export type OutboundMessage =
  | ReadyEvent
  | PongEvent
  | SessionReadyEvent
  | SdkEventForward
  | SessionErrorEvent;

// ----- Type guards -----

export function isInboundMessage(value: unknown): value is InboundMessage {
  if (typeof value !== 'object' || value === null) return false;
  const t = (value as { type?: unknown }).type;
  return (
    t === 'ping' ||
    t === 'create_session' ||
    t === 'send_message' ||
    t === 'close_session'
  );
}
