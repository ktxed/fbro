import { useEffect, useRef, useState } from "react";

import { useAppDispatch, useAppState, markHistoryNavPending, type PendingPanelHistory } from "../store";
import type { WsClient } from "../ws/client";

interface PanelHistoryModalProps {
  client: WsClient;
  panelHistory: PendingPanelHistory;
}

function entryLabel(path: string, sourceId: string): { title: string; subtitle: string } {
  if (sourceId.startsWith("zip:") || sourceId.startsWith("7z:")) {
    const archivePath = sourceId.startsWith("zip:") ? sourceId.slice(4) : sourceId.slice(3);
    const archiveName = archivePath.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? archivePath;
    const subPath = path ? path.replace(/^\//, "") : "";
    const title = subPath ? subPath : archiveName;
    return { title, subtitle: `\u{1F4E6} ${archiveName}` };
  }
  if (sourceId.startsWith("search:")) {
    return { title: path || "/", subtitle: "\u{1F50D} search results" };
  }
  const leaf = path.replace(/[/\\]+$/, "").split(/[/\\]/).filter(Boolean).pop() ?? path;
  return { title: leaf || "/", subtitle: path || "/" };
}

export function PanelHistoryModal({ client, panelHistory }: PanelHistoryModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const hist = state.panelHistory[panelHistory.panelId];
  const entries = hist?.entries ?? [];
  const cursor = hist?.cursor ?? 0;

  const [selectedIndex, setSelectedIndex] = useState(cursor);

  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  // Scroll selected item into view
  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  // Auto-close when panel path changes (navigation completed or cancelled)
  useEffect(() => {
    const panel = state.panels[panelHistory.panelId];
    if (!panel || panel.path !== panelHistory.currentPath) {
      dispatch({ type: "CLOSE_PANEL_HISTORY" });
    }
  }, [dispatch, panelHistory.currentPath, panelHistory.panelId, state.panels]);

  const close = () => dispatch({ type: "CLOSE_PANEL_HISTORY" });

  const navigateToEntry = (entryIndex: number) => {
    const entry = entries[entryIndex];
    if (!entry) return;
    dispatch({ type: "NAV_HISTORY_JUMP", panelId: panelHistory.panelId, entryIndex });
    markHistoryNavPending(panelHistory.panelId);
    client.sendIntent("NavigateToHistoryEntry", undefined, {
      panelId: panelHistory.panelId,
      path: entry.path,
      sourceId: entry.sourceId,
    });
    dispatch({ type: "CLOSE_PANEL_HISTORY" });
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
      if (entries.length > 0) setSelectedIndex(i => Math.min(i + 1, entries.length - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (entries.length > 0) setSelectedIndex(i => Math.max(i - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      navigateToEntry(selectedIndex);
    }
  };

  return (
    <div className="modal-overlay" onClick={close}>
      <div
        ref={dialogRef}
        className="modal command-palette open-with panel-history"
        role="dialog"
        aria-modal="true"
        aria-label="Navigation History"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <div className="modal__header">Navigation History</div>
        <div className="modal__body open-with__body">
          <div className="command-palette__list open-with__list" ref={listRef}>
            {entries.length === 0 && (
              <div className="command-palette__empty">No history yet.</div>
            )}
            {entries.map((entry, index) => {
              const isCurrent = index === cursor;
              const isBack = index < cursor;
              const selected = index === selectedIndex;
              const { title, subtitle } = entryLabel(entry.path, entry.sourceId);
              return (
                <button
                  key={`${entry.sourceId}::${entry.path}::${index}`}
                  type="button"
                  className={[
                    "command-palette__item",
                    selected ? "command-palette__item--selected" : "",
                    isCurrent ? "panel-history__item--current" : "",
                    isBack ? "panel-history__item--back" : "panel-history__item--forward",
                  ].filter(Boolean).join(" ")}
                  data-selected={selected ? "true" : "false"}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseEnter={() => setSelectedIndex(index)}
                  onClick={() => navigateToEntry(index)}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">
                      {isCurrent && <span className="panel-history__cursor-marker">→ </span>}
                      {title}
                    </span>
                    <span className="command-palette__item-category">{subtitle}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
