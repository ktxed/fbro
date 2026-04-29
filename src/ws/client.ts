import type { BEMessage, FEMessage, IntentMessage } from "./types";

const WS_URL = "ws://127.0.0.1:7878";
const RECONNECT_DELAY_MS = 1500;
// Localhost handshakes should complete in <50ms. A short timeout means a
// failed attempt unwinds quickly so the fast-retry path can kick in.
const CONNECT_TIMEOUT_MS = 500;

export class WsClient {
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private connectTimer: ReturnType<typeof setTimeout> | null = null;
  private destroyed = false;
  private clientKind: "gui" | "viewer";
  private pending: (FEMessage | IntentMessage)[] = [];

  constructor(clientKind: "gui" | "viewer" = "gui") {
    this.clientKind = clientKind;
  }

  onMessage: (msg: BEMessage) => void = () => {};
  onConnected: () => void = () => {};
  onDisconnected: () => void = () => {};

  connect() {
    if (this.destroyed) return;
    console.log("WS CONNECT", this.clientKind, WS_URL);
    let ws: WebSocket;
    try {
      ws = new WebSocket(WS_URL);
    } catch (e) {
      console.log("WS CONNECT THROW", this.clientKind, e);
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;

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
