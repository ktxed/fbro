import { Panel } from "./Panel";
import { OperationModal } from "./OperationModal";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";

interface LayoutProps {
  client: WsClient;
}

export function Layout({ client }: LayoutProps) {
  const state = useAppState();

  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);

  if (panelIds.length === 0) {
    return <div className="layout__empty">Connecting…</div>;
  }

  const activeJob = state.activeJobId ? state.jobs[state.activeJobId] ?? null : null;

  return (
    <>
      <div className="layout">
        {panelIds.map((id) => {
          const panel = state.panels[id];
          if (!panel) return null;
          return (
            <Panel
              key={id}
              panelId={id}
              panel={panel}
              isFocused={state.focus === id}
              client={client}
            />
          );
        })}
      </div>
      <OperationModal
        pending={state.pendingOp}
        activeJob={activeJob}
        activeJobId={state.activeJobId}
        client={client}
      />
    </>
  );
}
