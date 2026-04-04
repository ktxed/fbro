import { useEffect, useMemo, useRef, useState } from "react";

import { useAppDispatch, useAppState, type PendingPanelHistory } from "../store";
import type { WsClient } from "../ws/client";

interface PanelHistoryModalProps {
  client: WsClient;
  panelHistory: PendingPanelHistory;
}

export function PanelHistoryModal({ client, panelHistory }: PanelHistoryModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);

  const entries = useMemo(() => {
    const history = state.panelHistory[panelHistory.panelId] ?? [];
    return history.filter((path) => path !== panelHistory.currentPath);
  }, [panelHistory.currentPath, panelHistory.panelId, state.panelHistory]);

  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  useEffect(() => {
    if (entries.length === 0) {
      setSelectedIndex(0);
      return;
    }
    setSelectedIndex((current) => Math.min(current, entries.length - 1));
  }, [entries]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [entries, selectedIndex]);

  useEffect(() => {
    const panel = state.panels[panelHistory.panelId];
    if (!panel || panel.path !== panelHistory.currentPath) {
      dispatch({ type: "CLOSE_PANEL_HISTORY" });
    }
  }, [dispatch, panelHistory.currentPath, panelHistory.panelId, state.panels]);

  const close = () => dispatch({ type: "CLOSE_PANEL_HISTORY" });

  const selectedPath = useMemo(
    () => entries[selectedIndex],
    [entries, selectedIndex],
  );

  const navigateToPath = (path: string) => {
    client.sendIntent("NavigatePanelTo", undefined, {
      panelId: panelHistory.panelId,
      path,
    });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (entries.length > 0) {
        setSelectedIndex((current) => (current + 1) % entries.length);
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (entries.length > 0) {
        setSelectedIndex((current) => (current - 1 + entries.length) % entries.length);
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (selectedPath) navigateToPath(selectedPath);
    }
  };

  return (
    <div className="modal-overlay" onClick={close}>
      <div
        ref={dialogRef}
        className="modal command-palette open-with panel-history"
        role="dialog"
        aria-modal="true"
        aria-label="Panel History"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <div className="modal__header">Panel History</div>
        <div className="modal__body open-with__body">
          <div className="open-with__target" title={panelHistory.currentPath}>
            Current: {panelHistory.currentPath}
          </div>
          <div className="command-palette__list open-with__list" ref={listRef}>
            {entries.length === 0 && (
              <div className="command-palette__empty">No history yet.</div>
            )}
            {entries.map((path, index) => {
              const selected = index === selectedIndex;
              const leaf = path.replace(/[/\\]+$/, "").split(/[/\\]/).filter(Boolean).pop() ?? path;
              return (
                <button
                  key={path}
                  type="button"
                  className={`command-palette__item${selected ? " command-palette__item--selected" : ""}`}
                  data-selected={selected ? "true" : "false"}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseEnter={() => setSelectedIndex(index)}
                  onClick={() => navigateToPath(path)}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{leaf}</span>
                    <span className="command-palette__item-category">{path}</span>
                  </span>
                </button>
              );
            })}
          </div>
          {panelHistory.error && (
            <p className="modal__rename-error">{panelHistory.error}</p>
          )}
        </div>
      </div>
    </div>
  );
}
