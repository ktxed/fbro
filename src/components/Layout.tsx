import { Panel } from "./Panel";
import { OperationModal } from "./OperationModal";
import { QuitModal } from "./QuitModal";
import { RenameModal } from "./RenameModal";
import { InfoModal } from "./InfoModal";
import { MkDirModal } from "./MkDirModal";
import { NoEditorModal } from "./NoEditorModal";
import { JobsModal } from "./JobsModal";
import { CommandPaletteModal } from "./CommandPaletteModal";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";

interface LayoutProps {
  client: WsClient;
  jobsViewOpen: boolean;
  onCloseJobsView: () => void;
  onOpenJobsView: () => void;
}

export function Layout({ client, jobsViewOpen, onCloseJobsView, onOpenJobsView }: LayoutProps) {
  const state = useAppState();

  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);

  if (panelIds.length === 0) {
    return <div className="layout__empty">Connecting…</div>;
  }

  const activeJob = state.activeJobId ? state.jobs[state.activeJobId] ?? null : null;
  const modalActive = state.pendingOp !== null || state.activeJobId !== null || state.pendingQuit !== null || state.pendingRename !== null || state.pendingInfo !== null || state.pendingMkdir !== null || state.pendingNoEditor !== null || state.pendingDryRun !== null || state.commandPaletteOpen || jobsViewOpen;

  return (
    <>
      <div className="layout">
        {panelIds.map((id) => {
          const panel = state.panels[id];
          if (!panel) return null;
          const otherPanelId = panelIds.find(p => p !== id) ?? "";
          return (
            <Panel
              key={id}
              panelId={id}
              panel={panel}
              isFocused={state.focus === id}
              modalActive={modalActive}
              client={client}
              sources={state.sources}
              otherPanelId={otherPanelId}
            />
          );
        })}
      </div>
      <OperationModal
        pending={state.pendingOp}
        activeJob={activeJob}
        activeJobId={state.activeJobId}
        pendingDryRun={state.pendingDryRun}
        panelIds={panelIds}
        client={client}
      />
      {state.pendingQuit && (
        <QuitModal client={client} />
      )}
      {state.pendingRename && (
        <RenameModal rename={state.pendingRename} client={client} />
      )}
      {state.pendingInfo && (
        <InfoModal info={state.pendingInfo} />
      )}
      {state.pendingMkdir && (
        <MkDirModal mkdir={state.pendingMkdir} client={client} />
      )}
      {state.pendingNoEditor && (
        <NoEditorModal noEditor={state.pendingNoEditor} />
      )}
      {jobsViewOpen && (
        <JobsModal onClose={onCloseJobsView} client={client} />
      )}
      {state.commandPaletteOpen && (
        <CommandPaletteModal client={client} onOpenJobsView={onOpenJobsView} />
      )}
    </>
  );
}
