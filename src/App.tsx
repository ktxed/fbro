import { useEffect, useRef, useState } from "react";
import { AppContext, useAppReducer } from "./store";
import { WsClient } from "./ws/client";
import { Layout } from "./components/Layout";
import { JobsBar } from "./components/JobsBar";
import { Toolbar } from "./components/Toolbar";

export function App() {
  const [state, dispatch] = useAppReducer();
  const clientRef = useRef<WsClient | null>(null);
  const [jobsViewOpen, setJobsViewOpen] = useState(false);

  useEffect(() => {
    const client = new WsClient();
    clientRef.current = client;

    client.onConnected = () => dispatch({ type: "CONNECTED" });
    client.onDisconnected = () => dispatch({ type: "DISCONNECTED" });
    client.onMessage = (msg) => dispatch({ type: "BE_MESSAGE", msg });

    client.connect();
    return () => client.destroy();
  }, [dispatch]);

  const client = clientRef.current;

  return (
    <AppContext.Provider value={{ state, dispatch }}>
      {!state.connected && (
        <div className="banner banner--disconnected">
          Connecting to backend…
        </div>
      )}
      <div className="app">
        {client && <Layout client={client} jobsViewOpen={jobsViewOpen} onCloseJobsView={() => setJobsViewOpen(false)} onOpenJobsView={() => setJobsViewOpen(true)} />}
        {client && <JobsBar client={client} />}
        {client && <Toolbar client={client} jobsViewOpen={jobsViewOpen} onOpenJobsView={() => setJobsViewOpen(true)} />}
      </div>
    </AppContext.Provider>
  );
}
