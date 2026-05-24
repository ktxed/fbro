import type { BEMessage, FEMessage, IntentMessage } from "./types";
import { invoke } from "@tauri-apps/api/core";

const DEFAULT_WS_URL = "ws://127.0.0.1:7878";
const RECONNECT_DELAY_MS = 1500;
// Localhost handshakes should complete in <50ms. A short timeout means a
// failed attempt unwinds quickly so the fast-retry path can kick in.
const CONNECT_TIMEOUT_MS = 500;

type TerminalOutputListener = (bytes: Uint8Array) => void;

type FrontendConfig = {
  wsUrl?: string | null;
};

export async function resolveWsUrl() {
  try {
    const config = await invoke<FrontendConfig>("frontend_config");
    if (config.wsUrl) return config.wsUrl;
  } catch {
    // Browser-only/unit-test contexts do not have Tauri invoke.
  }
  try {
    const stored = window.localStorage.getItem("fbro.wsUrl");
    if (stored) return stored;
  } catch {
    // Ignore storage access failures.
  }
  return DEFAULT_WS_URL;
}

export class WsClient {
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private connectTimer: ReturnType<typeof setTimeout> | null = null;
  private destroyed = false;
  private clientKind: "gui" | "viewer";
  private wsUrl: string;
  private pending: (FEMessage | IntentMessage)[] = [];
  private terminalListeners = new Map<number, TerminalOutputListener>();

  constructor(clientKind: "gui" | "viewer" = "gui", wsUrl = DEFAULT_WS_URL) {
    this.clientKind = clientKind;
    this.wsUrl = wsUrl;
  }

  onMessage: (msg: BEMessage) => void = () => {};
  onConnected: () => void = () => {};
  onDisconnected: () => void = () => {};

  subscribeTerminalOutput(sessionId: number, cb: TerminalOutputListener) {
    this.terminalListeners.set(sessionId, cb);
  }

  unsubscribeTerminalOutput(sessionId: number) {
    this.terminalListeners.delete(sessionId);
  }

  /** Send raw input bytes to a PTY session via a binary WS frame.
   *  Layout: [tag:u8 = 0x02][session_id:u32-le][payload...] */
  sendTerminalInput(sessionId: number, bytes: Uint8Array) {
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    const frame = new Uint8Array(5 + bytes.length);
    frame[0] = 0x02;
    frame[1] = sessionId & 0xff;
    frame[2] = (sessionId >>> 8) & 0xff;
    frame[3] = (sessionId >>> 16) & 0xff;
    frame[4] = (sessionId >>> 24) & 0xff;
    frame.set(bytes, 5);
    this.ws.send(frame);
  }

  connect() {
    if (this.destroyed) return;
    console.log("WS CONNECT", this.clientKind, this.wsUrl);
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.wsUrl);
    } catch (e) {
      console.log("WS CONNECT THROW", this.clientKind, e);
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.binaryType = "arraybuffer";

    // Force-close if the handshake stalls — onclose will fire and trigger reconnect.
    this.connectTimer = setTimeout(() => {
      if (ws.readyState === WebSocket.CONNECTING) {
        try { ws.close(); } catch { /* ignore */ }
      }
    }, CONNECT_TIMEOUT_MS);

    ws.onopen = () => {
      console.log("WS OPEN", this.clientKind);
      if (this.connectTimer) { clearTimeout(this.connectTimer); this.connectTimer = null; }
      this.send({
        type: "Hello",
        timestamp: Date.now(),
        protocolVersion: "0",
        client: { kind: this.clientKind, name: "fbro-ui", supports: ["jobs", "decisions"] },
      });
      const queued = this.pending;
      this.pending = [];
      for (const msg of queued) this.send(msg);
      this.onConnected();
    };

    ws.onmessage = (event) => {
      // Binary frames carry terminal output (high-frequency, opaque bytes).
      // Layout: [tag:u8 = 0x01][session_id:u32-le][payload...]
      if (event.data instanceof ArrayBuffer) {
        const view = new Uint8Array(event.data);
        if (view.length >= 5 && view[0] === 0x01) {
          const sid = view[1] | (view[2] << 8) | (view[3] << 16) | (view[4] << 24);
          const listener = this.terminalListeners.get(sid >>> 0);
          if (listener) {
            // Pass the payload slice without copying.
            listener(view.subarray(5));
          }
        }
        return;
      }
      try {
        const msg = JSON.parse(event.data as string) as BEMessage;
        this.onMessage(msg);
      } catch (e) {
        console.error("Failed to parse BE message", e);
      }
    };

    ws.onclose = (e) => {
      console.log("WS CLOSE", this.clientKind, e.code, e.reason);
      if (this.connectTimer) { clearTimeout(this.connectTimer); this.connectTimer = null; }
      this.onDisconnected();
      // Code 1006 is a transient failure (server busy / handshake not yet ready).
      // Retry quickly rather than waiting the full reconnect delay.
      this.scheduleReconnect(e.code === 1006 ? 50 : RECONNECT_DELAY_MS);
    };

    ws.onerror = () => {
      console.log("WS ERROR", this.clientKind, "readyState=", this.ws?.readyState);
      // onclose fires after onerror, reconnect is handled there
    };
  }

  sendIntent(
    name: string,
    scope?: string | { from?: string; to?: string },
    args?: Record<string, unknown>,
    options?: Record<string, unknown>,
  ) {
    const msg: IntentMessage = {
      type: "Intent",
      timestamp: Date.now(),
      name,
      scope,
      args,
      options,
    };
    this.send(msg);
  }

  private send(msg: FEMessage | IntentMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
      return;
    }
    if (msg.type === "Hello") return;
    console.warn("WS QUEUE", this.clientKind, "readyState=", this.ws?.readyState, msg);
    if (this.pending.length >= 32) this.pending.shift();
    this.pending.push(msg);
  }

  private scheduleReconnect(delay = RECONNECT_DELAY_MS) {
    if (this.destroyed) return;
    this.reconnectTimer = setTimeout(() => {
      this.connect();
    }, delay);
  }

  destroy() {
    this.destroyed = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    if (this.connectTimer) clearTimeout(this.connectTimer);
    this.ws?.close();
  }
}
