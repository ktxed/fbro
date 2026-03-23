import type { BEMessage, FEMessage, IntentMessage } from "./types";

const WS_URL = "ws://127.0.0.1:7878";
const RECONNECT_DELAY_MS = 1500;

export class WsClient {
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private destroyed = false;

  onMessage: (msg: BEMessage) => void = () => {};
  onConnected: () => void = () => {};
  onDisconnected: () => void = () => {};

  connect() {
    if (this.destroyed) return;
    try {
      this.ws = new WebSocket(WS_URL);
    } catch {
      this.scheduleReconnect();
      return;
    }

    this.ws.onopen = () => {
      this.send({
        type: "Hello",
        timestamp: new Date().toISOString(),
        protocolVersion: "0",
        client: { kind: "gui", name: "fbro-ui", supports: ["jobs", "decisions"] },
      });
      this.onConnected();
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data as string) as BEMessage;
        this.onMessage(msg);
      } catch (e) {
        console.error("Failed to parse BE message", e);
      }
    };

    this.ws.onclose = () => {
      this.onDisconnected();
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
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
      timestamp: new Date().toISOString(),
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
    }
  }

  private scheduleReconnect() {
    if (this.destroyed) return;
    this.reconnectTimer = setTimeout(() => {
      this.connect();
    }, RECONNECT_DELAY_MS);
  }

  destroy() {
    this.destroyed = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }
}
