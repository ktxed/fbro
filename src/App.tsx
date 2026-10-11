import { useEffect, useRef, useState } from "react";
import { AppContext, useAppReducer } from "./store";
import { resolveWsUrl, WsClient } from "./ws/client";
import { Layout } from "./components/Layout";
import { JobsBar } from "./components/JobsBar";
import { Toolbar } from "./components/Toolbar";
import { applyZoom } from "./utils/zoom";
import { resolveColorScheme } from "./theme/colorSchemes";
import { ModalStackProvider } from "./components/ModalStack";
import { useNotificationBridge } from "./notifications/service";

function NotificationBridge({ client }: { client: WsClient | null }) {
  useNotificationBridge(client);
  return null;
}

export function App() {
  const [state, dispatch] = useAppReducer();
  const clientRef = useRef<WsClient | null>(null);
  const [jobsViewOpen, setJobsViewOpen] = useState(false);
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);

  useEffect(() => { applyZoom(state.fontSize); }, [state.fontSize]);

  // Latest discoveredPeers, captured via a ref so the onMessage closure
  // doesn't need to be re-created (and the WS doesn't need to be torn
  // down) every time the peer list changes.
  const discoveredRef = useRef(state.discoveredPeers);
  useEffect(() => {
    discoveredRef.current = state.discoveredPeers;
  }, [state.discoveredPeers]);

  useEffect(() => {
    let cancelled = false;

    resolveWsUrl().then((wsUrl) => {
      if (cancelled) return;
      const client = new WsClient("gui", wsUrl);
      clientRef.current = client;

      client.onConnected = () => dispatch({ type: "CONNECTED" });
      client.onDisconnected = () => dispatch({ type: "DISCONNECTED" });
      client.onMessage = (msg) => {
        dispatch({ type: "BE_MESSAGE", msg, notificationRequestId: crypto.randomUUID() });
        // Side-effect: once a pairing handshake completes successfully,
        // auto-mount the remote sources. The BE emits a `discoveredPeers`
        // patch immediately before `PairingResolved`, so by this point our
        // ref already reflects the freshly-paired peer.
        if (msg.type === "PairingResolved" && msg.outcome === "paired" && msg.fingerprint) {
          const fp = msg.fingerprint;
          const peer = discoveredRef.current.find((p) => p.pubkeyFp === fp);
          if (peer) {
            client.sendIntent("ConnectRemotePeer", undefined, { peerId: peer.id });
          } else {
            client.sendIntent("RetryLastPeer", undefined, { fingerprint: fp });
          }
        }
      };

      client.connect();
    });
    return () => {
      cancelled = true;
      clientRef.current?.destroy();
      clientRef.current = null;
    };
  }, [dispatch]);

  const client = clientRef.current;

  useEffect(() => {
    if (!client) return;
    const refreshGitStatus = () => client.sendIntent("RefreshGitStatus");
    window.addEventListener("focus", refreshGitStatus);
    return () => window.removeEventListener("focus", refreshGitStatus);
  }, [client]);

  return (
    <AppContext.Provider value={{ state, dispatch }}>
      <NotificationBridge client={client} />
      <ModalStackProvider>
        {!state.connected && (
          <div className="banner banner--disconnected">
            Connecting to backend…
          </div>
        )}
        <div
          className="app"
          data-color-scheme={colorScheme.id}
          style={{ colorScheme: colorScheme.mode }}
        >
          {client && <Layout client={client} jobsViewOpen={jobsViewOpen} onCloseJobsView={() => setJobsViewOpen(false)} onOpenJobsView={() => setJobsViewOpen(true)} />}
          {client && <JobsBar client={client} onOpenJobsView={() => setJobsViewOpen(true)} />}
          {client && <Toolbar client={client} onOpenJobsView={() => setJobsViewOpen(true)} />}
        </div>
      </ModalStackProvider>
    </AppContext.Provider>
  );
}
