import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Panel } from "./Panel";
import { PanelErrorBoundary } from "./PanelErrorBoundary";
import { OperationModal } from "./OperationModal";
import { QuitModal } from "./QuitModal";
import { RenameModal } from "./RenameModal";
import { AttributesModal } from "./AttributesModal";
import { InfoModal } from "./InfoModal";
import { MkDirModal } from "./MkDirModal";
import { TouchModal } from "./TouchModal";
import { NoEditorModal } from "./NoEditorModal";
import { GoToPathModal } from "./GoToPathModal";
import { GlobalSelectionModal } from "./GlobalSelectionModal";
import { PanelHistoryModal } from "./PanelHistoryModal";
import { JobsModal } from "./JobsModal";
import { CommandPaletteModal } from "./CommandPaletteModal";
import { AboutModal } from "./AboutModal";
import { ContextMenu } from "./ContextMenu";
import { SourceContextMenu } from "./SourceContextMenu";
import { EjectModal } from "./EjectModal";
import { OpenWithModal } from "./OpenWithModal";
import { BroModal } from "./BroModal";
import { InputModal } from "./InputModal";
import { SearchModal } from "./SearchModal";
import { SettingsModal } from "./SettingsModal";
import { useModalStack } from "./ModalStack";
import { useAppDispatch, useAppState } from "../store";
import { EmbeddedViewerPanel } from "./EmbeddedViewerPanel";
import { captureFocusReturnTarget, scheduleFocusReturn } from "../utils/focusReturn";
import type { FileOpFile, FileOpType, PanelState, VFSEntry } from "../ws/types";
import type { WsClient } from "../ws/client";

interface LayoutProps {
  client: WsClient;
  jobsViewOpen: boolean;
  onCloseJobsView: () => void;
  onOpenJobsView: () => void;
}

interface DragCandidate {
  sourcePanelId: string;
  sourcePath: string;
  absIndex: number;
  clientX: number;
  clientY: number;
  files: FileOpFile[] | null;
  sourceAbsIndices: number[] | null;
}

interface ActiveDragSession {
  sourcePanelId: string;
  sourcePath: string;
  anchorAbsIndex: number;
  files: FileOpFile[];
  sourceAbsIndices: number[];
  sourceNames: string[];
  pointerX: number;
  pointerY: number;
  hoverPanelId: string | null;
  hoverAbsIndex: number | null;
  dropDestPath: string | null;
  dropAllowed: boolean;
  opType: FileOpType;
}

const DRAG_THRESHOLD_PX = 6;

function normalizeDirPath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalized || "/";
}

function joinChildPath(dirPath: string, name: string): string {
  const base = normalizeDirPath(dirPath);
  return base === "/" ? `/${name}` : `${base}/${name}`;
}

function getEntryAtAbsIndex(panel: PanelState, absIndex: number): VFSEntry | null {
  const isWindowed = panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
  if (isWindowed) {
    const windowStart = panel.windowStart ?? 0;
    if (absIndex < windowStart || absIndex >= windowStart + panel.entries.length) return null;
    return panel.entries[absIndex - windowStart] ?? null;
  }
  return panel.entries[absIndex] ?? null;
}

function resolveDropTarget(
  drag: ActiveDragSession,
  panels: Record<string, PanelState>,
  clientX: number,
  clientY: number,
) {
  const hoveredElement = document.elementFromPoint(clientX, clientY);
  const panelElement = hoveredElement?.closest<HTMLElement>("[data-panel-id]");
  const hoverPanelId = panelElement?.dataset.panelId ?? null;

  if (!hoverPanelId || hoverPanelId === drag.sourcePanelId) {
    return {
      hoverPanelId,
      hoverAbsIndex: null,
      dropDestPath: null,
      dropAllowed: false,
    };
  }

  const targetPanel = panels[hoverPanelId];
  if (!targetPanel) {
    return {
      hoverPanelId,
      hoverAbsIndex: null,
      dropDestPath: null,
      dropAllowed: false,
    };
  }

  const rowElement = hoveredElement?.closest<HTMLElement>("[data-abs-index]");
  const hoverAbsIndex = rowElement ? Number.parseInt(rowElement.dataset.absIndex ?? "", 10) : null;
  const resolvedHoverAbsIndex = hoverAbsIndex !== null && Number.isFinite(hoverAbsIndex) ? hoverAbsIndex : null;
  const samePath = normalizeDirPath(targetPanel.path) === drag.sourcePath;

  if (!samePath) {
    return {
      hoverPanelId,
      hoverAbsIndex: resolvedHoverAbsIndex,
      dropDestPath: targetPanel.path,
      dropAllowed: true,
    };
  }

  const hoverEntry = resolvedHoverAbsIndex !== null ? getEntryAtAbsIndex(targetPanel, resolvedHoverAbsIndex) : null;
  const dropAllowed = !!hoverEntry
    && hoverEntry.kind === "directory"
    && hoverEntry.name !== ".."
    && !drag.sourceNames.includes(hoverEntry.name);

  return {
    hoverPanelId,
    hoverAbsIndex: resolvedHoverAbsIndex,
    dropDestPath: dropAllowed && hoverEntry ? joinChildPath(targetPanel.path, hoverEntry.name) : null,
    dropAllowed,
  };
}

export function Layout({ client, jobsViewOpen, onCloseJobsView, onOpenJobsView }: LayoutProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const { hasActiveModal } = useModalStack();
  const [dragCandidate, setDragCandidate] = useState<DragCandidate | null>(null);
  const [activeDrag, setActiveDrag] = useState<ActiveDragSession | null>(null);
  const [dragOutInProgress, setDragOutInProgress] = useState(false);
  const dragOutInProgressRef = useRef(false);
  const [externalDragHover, setExternalDragHover] = useState<{ panelId: string; absIndex: number | null } | null>(null);
  const externalDragPanelId = useRef<string | null>(null);
  const externalDragHoverIndex = useRef<number | null>(null);

  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);

  // Keep a ref to always-current state so the external drag-drop listener
  // (registered once) never reads stale closure values.
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  });

  // Notify BE when the embedded viewer opens or closes so it can gate
  // background prefetch on viewer presence.
  const embeddedViewerActive = state.embeddedViewer !== null;
  useEffect(() => {
    client.sendIntent("SetEmbeddedViewerActive", undefined, { active: embeddedViewerActive });
  }, [client, embeddedViewerActive]);

  useEffect(() => {
    if (!dragCandidate && !activeDrag) return;

    const handleMouseMove = (event: MouseEvent) => {
      setActiveDrag((current) => {
        if (current) {
          const nextOpType: FileOpType = event.shiftKey ? "move" : "copy";
          const drop = resolveDropTarget(current, state.panels, event.clientX, event.clientY);
          return {
            ...current,
            pointerX: event.clientX,
            pointerY: event.clientY,
            opType: nextOpType,
            ...drop,
          };
        }

        if (!dragCandidate) return null;

        const distance = Math.hypot(event.clientX - dragCandidate.clientX, event.clientY - dragCandidate.clientY);
        if (distance < DRAG_THRESHOLD_PX || !dragCandidate.files || !dragCandidate.sourceAbsIndices) {
          return null;
        }

        const nextOpType: FileOpType = event.shiftKey ? "move" : "copy";
        const nextDrag: ActiveDragSession = {
          sourcePanelId: dragCandidate.sourcePanelId,
          sourcePath: dragCandidate.sourcePath,
          anchorAbsIndex: dragCandidate.absIndex,
          files: dragCandidate.files,
          sourceAbsIndices: dragCandidate.sourceAbsIndices,
          sourceNames: dragCandidate.files.map((file) => file.name),
          pointerX: event.clientX,
          pointerY: event.clientY,
          hoverPanelId: null,
          hoverAbsIndex: null,
          dropDestPath: null,
          dropAllowed: false,
          opType: nextOpType,
        };
        return { ...nextDrag, ...resolveDropTarget(nextDrag, state.panels, event.clientX, event.clientY) };
      });
    };

    const handleMouseUp = () => {
      if (dragOutInProgressRef.current) {
        setDragCandidate(null);
        return;
      }

      if (activeDrag) {
        if (activeDrag.dropAllowed && activeDrag.hoverPanelId && activeDrag.dropDestPath) {
          dispatch({
            type: "OPEN_OPERATION",
            op: {
              opType: activeDrag.opType,
              files: activeDrag.files,
              sourcePanel: activeDrag.sourcePanelId,
              destPanel: activeDrag.hoverPanelId,
              destPath: activeDrag.dropDestPath,
              conflictPolicy: "ask",
            },
          });
        } else {
          dispatch({ type: "CLOSE_CONTEXT_MENU" });
        }
      } else if (dragCandidate) {
        dispatch({ type: "CLOSE_CONTEXT_MENU" });
      }

      setDragCandidate(null);
      setActiveDrag(null);
    };

    const handleShiftChange = (event: KeyboardEvent) => {
      if (event.key !== "Shift") return;
      setActiveDrag((current) => current ? { ...current, opType: event.type === "keydown" ? "move" : "copy" } : current);
    };

    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
    window.addEventListener("keydown", handleShiftChange);
    window.addEventListener("keyup", handleShiftChange);

    return () => {
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
      window.removeEventListener("keydown", handleShiftChange);
      window.removeEventListener("keyup", handleShiftChange);
    };
  }, [activeDrag, client, dispatch, dragCandidate, state.panels]);

  useEffect(() => {
    if (!state.transientHint) return;
    const timer = window.setTimeout(() => {
      dispatch({ type: "CLOSE_TRANSIENT_HINT" });
    }, 2400);
    return () => window.clearTimeout(timer);
  }, [dispatch, state.transientHint]);

  useEffect(() => {
    const unlisten = listen<{ success: boolean }>("drag-out-complete", () => {
      setDragOutInProgress(false);
      dragOutInProgressRef.current = false;
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  const handleLayoutMouseLeave = useCallback(() => {
    if (!activeDrag || activeDrag.files.length === 0 || dragOutInProgressRef.current) return;

    const paths = activeDrag.files.map(f =>
      f.sourcePath ?? `${activeDrag.sourcePath}/${f.name}`
    );

    setDragOutInProgress(true);
    dragOutInProgressRef.current = true;
    setActiveDrag(null);
    setDragCandidate(null);

    invoke("start_drag_out", { paths }).catch(() => {
      setDragOutInProgress(false);
      dragOutInProgressRef.current = false;
    });
  }, [activeDrag]);

  const handleExternalDrop = useCallback((panelId: string, files: FileOpFile[], destPath: string) => {
    console.log("[fbro] handleExternalDrop called:", { panelId, files, destPath });
    setExternalDragHover(null);
    externalDragPanelId.current = null;
    externalDragHoverIndex.current = null;
    dispatch({
      type: "OPEN_OPERATION",
      op: {
        opType: "copy",
        files,
        sourcePanel: panelId,
        destPanel: panelId,
        destPath,
        conflictPolicy: "ask",
      },
    });
  }, [dispatch]);

  useEffect(() => {
    const webview = getCurrentWebview();
    
    const findPanelAtPosition = (position: { x: number; y: number }): { panelId: string; absIndex: number | null } | null => {
      const hoveredElement = document.elementFromPoint(position.x, position.y);
      const panelElement = hoveredElement?.closest<HTMLElement>("[data-panel-id]");
      const panelId = panelElement?.dataset.panelId ?? null;
      
      if (panelId) {
        const rowElement = hoveredElement?.closest<HTMLElement>("[data-abs-index]");
        const absIndex = rowElement ? Number.parseInt(rowElement.dataset.absIndex ?? "", 10) : null;
        return { panelId, absIndex: Number.isFinite(absIndex) ? absIndex : null };
      }
      return null;
    };

    const handleDragOver = (position: { x: number; y: number }) => {
      const result = findPanelAtPosition(position);
      if (result) {
        externalDragPanelId.current = result.panelId;
        externalDragHoverIndex.current = result.absIndex;
        setExternalDragHover({ panelId: result.panelId, absIndex: result.absIndex });
      } else {
        setExternalDragHover(null);
        externalDragPanelId.current = null;
        externalDragHoverIndex.current = null;
      }
    };

    const handleDrop = async (paths: string[], position: { x: number; y: number }) => {
      if (paths.length > 0) {
        const files: FileOpFile[] = [];
        for (const path of paths) {
          try {
            const info = await invoke<{ kind: string; size: number | null }>("get_path_info", { path });
            files.push({
              name: path.split(/[/\\]/).pop() ?? path,
              kind: info.kind,
              size: info.size ?? undefined,
              sourcePath: path,
            });
          } catch {
            files.push({
              name: path.split(/[/\\]/).pop() ?? path,
              kind: "file",
              sourcePath: path,
            });
          }
        }
        
        const result = findPanelAtPosition(position);
        const currentState = stateRef.current;
        const panelId = result?.panelId ?? currentState.focus;
        const panel = panelId ? currentState.panels[panelId] : null;
        
        let destPath = panel?.path ?? "/";
        
        if (result?.absIndex !== null && result?.absIndex !== undefined) {
          const isWindowed = panel && panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
          const entryIndex = isWindowed ? result.absIndex - (panel?.windowStart ?? 0) : result.absIndex;
          const entry = panel?.entries[entryIndex];
          if (entry && entry.kind === "directory" && entry.name !== "..") {
            destPath = `${panel.path.replace(/[/\\]+$/, "")}/${entry.name}`;
          }
        }
        
        console.log("[fbro] using panelId:", panelId, "destPath:", destPath);
        
        if (panelId) {
          handleExternalDrop(panelId, files, destPath);
        }
      }
      setExternalDragHover(null);
      externalDragPanelId.current = null;
      externalDragHoverIndex.current = null;
    };

    const handleDragCancel = () => {
      setExternalDragHover(null);
      externalDragPanelId.current = null;
      externalDragHoverIndex.current = null;
    };

    let unlisten: (() => void) | null = null;
    let cancelled = false;

    webview.onDragDropEvent((event) => {
      console.log("[fbro] drag drop event:", event.payload.type);
      if (event.payload.type === "over") {
        handleDragOver(event.payload.position);
      } else if (event.payload.type === "drop") {
        handleDrop(event.payload.paths, event.payload.position);
      } else {
        handleDragCancel();
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [handleExternalDrop]);

  if (panelIds.length === 0) {
    return <div className="layout__empty">Connecting…</div>;
  }

  const activeJob = state.activeJobId ? state.jobs[state.activeJobId] ?? null : null;
  const modalActive = hasActiveModal || state.pendingContextMenu !== null || state.pendingSourceMenu !== null || state.bookmarksOpen !== null;

  const dragCursorClass = dragOutInProgress
    ? "layout--drag-out"
    : activeDrag
      ? activeDrag.dropAllowed
        ? activeDrag.opType === "move"
          ? "layout--drag-move"
          : "layout--drag-copy"
        : "layout--drag-no-drop"
      : "";

  return (
    <>
      <div className={`layout ${dragCursorClass}`.trim()} onMouseLeave={handleLayoutMouseLeave}>
        {panelIds.map((id) => {
          const panel = state.panels[id];
          if (!panel) return null;
          const otherPanelId = panelIds.find(p => p !== id) ?? "";

          // In embedded viewer mode, panels in slots other than the source slot
          // become preview panes. The source panel resolves to whichever tab is
          // currently active in the source slot.
          if (state.embeddedViewer && panel.slotId !== state.embeddedViewer.sourceSlotId) {
            const activeSourcePanelId =
              state.slots[state.embeddedViewer.sourceSlotId]?.activePanelId;
            if (activeSourcePanelId) {
              return (
                <EmbeddedViewerPanel
                  key={id}
                  sourcePanelId={activeSourcePanelId}
                  state={state}
                  onClose={() => {
                    const target = captureFocusReturnTarget(state.focus);
                    dispatch({ type: "TOGGLE_EMBEDDED_VIEWER" });
                    scheduleFocusReturn(target);
                  }}
                />
              );
            }
          }

          return (
            <PanelErrorBoundary
              key={id}
              panelId={id}
              client={client}
              resetKey={`${panel.sourceId}:${panel.path}:${panel.version}`}
            >
              <Panel
                panelId={id}
                panel={panel}
                isFocused={state.focus === id}
                modalActive={modalActive}
                client={client}
                sources={state.sources}
                otherPanelId={otherPanelId}
                onRowPointerDown={({ absIndex, clientX, clientY }) => {
                  const currentPanel = state.panels[id];
                  const entry = currentPanel ? getEntryAtAbsIndex(currentPanel, absIndex) : null;
                  if (!currentPanel || !entry) return;

                  const normalizedPath = normalizeDirPath(currentPanel.path);
                  const hasSelection = currentPanel.selection.length > 0;
                  const rowSelected = currentPanel.selection.includes(absIndex);

                  let files: FileOpFile[] | null = null;
                  let sourceAbsIndices: number[] | null = null;

                  if (!hasSelection) {
                    if (entry.name !== "..") {
                      files = [{ name: entry.name, size: entry.size, kind: entry.kind, sourcePath: entry.sourcePath }];
                      sourceAbsIndices = [absIndex];
                    }
                  } else if (rowSelected) {
                    files = currentPanel.selectedEntries;
                    sourceAbsIndices = currentPanel.selection;
                  }

                  setActiveDrag(null);
                  setDragCandidate({
                    sourcePanelId: id,
                    sourcePath: normalizedPath,
                    absIndex,
                    clientX,
                    clientY,
                    files,
                    sourceAbsIndices,
                  });
                }}
                dragActive={activeDrag !== null}
                dragSource={activeDrag?.sourcePanelId === id}
                dragHovered={activeDrag?.hoverPanelId === id}
                dragDropAllowed={activeDrag?.hoverPanelId === id && activeDrag.dropAllowed}
                dragHoverAbsIndex={activeDrag?.hoverPanelId === id ? activeDrag.hoverAbsIndex : null}
                externalDragHovered={externalDragHover?.panelId === id}
              />
            </PanelErrorBoundary>
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
        <InfoModal info={state.pendingInfo} client={client} />
      )}
      {state.pendingAttributes && (
        <AttributesModal attributes={state.pendingAttributes} client={client} />
      )}
      {state.pendingMkdir && (
        <MkDirModal mkdir={state.pendingMkdir} client={client} />
      )}
      {state.pendingTouch && (
        <TouchModal touch={state.pendingTouch} client={client} />
      )}
      {state.pendingNoEditor && (
        <NoEditorModal noEditor={state.pendingNoEditor} />
      )}
      {state.pendingGoToPath && (
        <GoToPathModal goToPath={state.pendingGoToPath} client={client} />
      )}
      {state.pendingPanelHistory && (
        <PanelHistoryModal client={client} panelHistory={state.pendingPanelHistory} />
      )}
      {jobsViewOpen && (
        <JobsModal onClose={onCloseJobsView} client={client} />
      )}
      {state.pendingContextMenu && (
        <ContextMenu client={client} menu={state.pendingContextMenu} />
      )}
      {state.pendingSourceMenu && <SourceContextMenu menu={state.pendingSourceMenu} />}
      {state.pendingEject && <EjectModal client={client} />}
      {state.pendingGlobalSelectionAdd && (
        <GlobalSelectionModal client={client} add={state.pendingGlobalSelectionAdd} />
      )}
      {state.pendingOpenWith && (
        <OpenWithModal client={client} openWith={state.pendingOpenWith} />
      )}
      {state.pendingBro && (
        <BroModal client={client} bro={state.pendingBro} />
      )}
      {state.pendingInput && (
        <InputModal input={state.pendingInput} client={client} />
      )}
      {state.pendingSearch && (
        <SearchModal search={state.pendingSearch} sources={state.sources} client={client} />
      )}
      {state.commandPaletteOpen && (
        <CommandPaletteModal client={client} onOpenJobsView={onOpenJobsView} />
      )}
      {state.aboutOpen && (
        <AboutModal client={client} />
      )}
      {state.pendingSettings && (
        <SettingsModal client={client} />
      )}
      {state.transientHint && (
        <div className="transient-hint" data-testid="transient-hint" role="status" aria-live="polite"
          onMouseDown={(event) => event.preventDefault()}>
          {state.transientHint.message}
        </div>
      )}
    </>
  );
}
