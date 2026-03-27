import { useState, useEffect, useRef } from "react";
import type { PanelState, VFSEntry, Source } from "../ws/types";
import type { WsClient } from "../ws/client";

function SortHeader({ sort, panelId, client }: {
  sort: PanelState["sort"];
  panelId: string;
  client: WsClient;
}) {
  function handleSort(by: string) {
    const order = sort.by === by && sort.order === "asc" ? "desc" : "asc";
    client.sendIntent("SetSort", panelId, { by, order });
  }

  function tri(col: string) {
    const active = sort.by === col;
    const icon = active && sort.order === "desc" ? "▼" : "▲";
    return <span className={`panel__sort-tri${active ? " panel__sort-tri--active" : ""}`}>{icon}</span>;
  }

  return (
    <div className="panel__sort-header">
      <span className="panel__sort-kind" onClick={() => handleSort("kind")}>{tri("kind")}</span>
      <span className="panel__sort-name" onClick={() => handleSort("name")}>Name {tri("name")}</span>
      <span className="panel__sort-size" onClick={() => handleSort("size")}>Size {tri("size")}</span>
      <span className="panel__sort-date" onClick={() => handleSort("date")}>Date {tri("date")}</span>
      <span className="panel__sort-perms" />
    </div>
  );
}

interface PanelProps {
  panelId: string;
  panel: PanelState;
  isFocused: boolean;
  modalActive: boolean;
  client: WsClient;
  sources: Record<string, Source>;
}

function VolumeHeader({ sources, panel, panelId, client }: {
  sources: Record<string, Source>;
  panel: PanelState;
  panelId: string;
  client: WsClient;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);

  const updateArrows = () => {
    const el = listRef.current;
    if (!el) return;
    setCanLeft(el.scrollLeft > 0);
    setCanRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  };

  useEffect(() => {
    const id = setTimeout(updateArrows, 0);
    return () => clearTimeout(id);
  }, [sources]);

  const scroll = (dir: number) => {
    listRef.current?.scrollBy({ left: dir * 80, behavior: "smooth" });
  };

  return (
    <div className="panel__volumes">
      <button className="panel__volumes-arrow" disabled={!canLeft} tabIndex={-1} onClick={() => scroll(-1)}>‹</button>
      <div className="panel__volumes-list" ref={listRef} onScroll={updateArrows}>
        {Object.values(sources).map(src => (
          <button
            key={src.id}
            className={`panel__volume-btn${src.id === panel.sourceId ? " panel__volume-btn--active" : ""}`}
            tabIndex={-1}
            onClick={() => client.sendIntent("NavigateToSource", panelId, { sourceId: src.id })}
            title={src.rootPath}
          >{src.name}</button>
        ))}
      </div>
      <button className="panel__volumes-arrow" disabled={!canRight} tabIndex={-1} onClick={() => scroll(1)}>›</button>
    </div>
  );
}

function fallbackIcon(entry: VFSEntry): string {
  switch (entry.kind) {
    case "directory": return "▶";
    case "symlink":   return "↗";
    case "file":      return " ";
    default:          return "?";
  }
}

function EntryIcon({ entry }: { entry: VFSEntry }) {
  const [failed, setFailed] = useState(false);
  const iconName = entry.iconName;
  return (
    <span className="panel__col-icon">
      {iconName && !failed ? (
        <img
          src={`icon://localhost/${iconName}`}
          alt=""
          draggable={false}
          onError={() => setFailed(true)}
        />
      ) : (
        <span>{fallbackIcon(entry)}</span>
      )}
    </span>
  );
}

function formatSize(entry: VFSEntry): string {
  if (entry.kind === "directory") return "<DIR>";
  if (entry.size === undefined) return "";
  const s = entry.size;
  if (s < 1024) return `${s}`;
  if (s < 1024 * 1024) return `${(s / 1024).toFixed(1)}K`;
  if (s < 1024 * 1024 * 1024) return `${(s / (1024 * 1024)).toFixed(1)}M`;
  return `${(s / (1024 * 1024 * 1024)).toFixed(1)}G`;
}

function formatMtime(mtime: string | undefined): string {
  if (!mtime) return "";
  const secs = parseInt(mtime, 10);
  if (isNaN(secs)) return "";
  const d = new Date(secs * 1000);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function Panel({ panelId, panel, isFocused, modalActive, client, sources }: PanelProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const rightDrag = useRef({ active: false, everMoved: false, startRow: -1, mode: "select" as "select" | "deselect", preSelection: [] as number[] });

  // Keep focused panel's div focused so it receives keyboard events.
  // Also re-focus when a modal closes (modalActive transitions to false).
  useEffect(() => {
    if (isFocused && !modalActive) {
      focusDivRef.current?.focus();
    }
  }, [isFocused, modalActive]);

  // End right-drag on mouse up anywhere; if a drag occurred, send final SetSelection
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 2) {
        rightDrag.current.active = false;
      }
    };
    window.addEventListener("mouseup", onMouseUp);
    return () => window.removeEventListener("mouseup", onMouseUp);
  }, []);

  // Scroll cursor into view within panel__list.
  // Uses getBoundingClientRect so the row position is always relative to the
  // list viewport, regardless of which element is the CSS offsetParent.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.children[panel.cursor] as HTMLElement | undefined;
    if (!row) return;
    const listRect = list.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const rowTop = rowRect.top - listRect.top + list.scrollTop;
    const rowBottom = rowTop + row.offsetHeight;
    if (rowTop < list.scrollTop) {
      list.scrollTop = rowTop;
    } else if (rowBottom > list.scrollTop + list.clientHeight) {
      list.scrollTop = rowBottom - list.clientHeight;
    }
  }, [panel.cursor]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (modalActive) {
      e.preventDefault();
      return;
    }
    switch (e.key) {
      case "ArrowUp":
        e.preventDefault();
        if (e.metaKey) {
          client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
        } else if (e.shiftKey) {
          client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "up" });
        } else {
          client.sendIntent("MoveCursor", "focused-panel", { direction: "up" });
        }
        break;
      case "ArrowDown":
        e.preventDefault();
        if (e.metaKey) {
          client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
        } else if (e.shiftKey) {
          client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "down" });
        } else {
          client.sendIntent("MoveCursor", "focused-panel", { direction: "down" });
        }
        break;
      case " ":
        e.preventDefault();
        client.sendIntent("ToggleSelection", "focused-panel");
        break;
      case "a":
        if (e.ctrlKey) {
          e.preventDefault();
          client.sendIntent("SelectAll", "focused-panel");
        }
        break;
      case "PageUp":
      case "PageDown": {
        e.preventDefault();
        const list = listRef.current;
        if (!list) break;
        const firstRow = list.children[0] as HTMLElement | undefined;
        const rowHeight = firstRow ? firstRow.getBoundingClientRect().height : 1;
        const pageSize = Math.max(1, Math.floor(list.clientHeight / rowHeight));
        const delta = e.key === "PageDown" ? pageSize : -pageSize;
        client.sendIntent("MoveCursorBy", "focused-panel", { delta });
        break;
      }
      case "Home":
        e.preventDefault();
        client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
        break;
      case "End":
        e.preventDefault();
        client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
        break;
      case "Enter": {
        e.preventDefault();
        const entry = panel.entries[panel.cursor];
        if (entry && entry.kind !== "directory") {
          client.sendIntent("OpenFile", "focused-panel");
        } else {
          client.sendIntent("OpenEntry", "focused-panel");
        }
        break;
      }
      case "ArrowRight":
        e.preventDefault();
        client.sendIntent("OpenEntry", "focused-panel");
        break;
      case "Backspace":
      case "ArrowLeft":
        e.preventDefault();
        client.sendIntent("NavigateUp", "focused-panel");
        break;
      case "Tab":
        e.preventDefault();
        client.sendIntent("SwitchFocus", undefined, {
          to: panelId === "left" ? "right" : "left",
        });
        break;
    }
  }

  const statusText =
    panel.status === "loading"
      ? "Loading…"
      : `${panel.entries.length} item${panel.entries.length !== 1 ? "s" : ""}`;

  return (
    <div
      className={`panel ${isFocused ? "panel--focused" : ""}`}
      ref={focusDivRef}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onFocus={() => {
        if (!isFocused && !modalActive) {
          client.sendIntent("SwitchFocus", undefined, { to: panelId });
        }
      }}
    >
      <div className="panel__header">
        <span className="panel__path">{panel.path}</span>
        <button
          className="panel__home-btn"
          tabIndex={-1}
          title="Navigate to home folder"
          onClick={() => client.sendIntent("NavigateHome", panelId)}
        >~</button>
      </div>

      <VolumeHeader sources={sources} panel={panel} panelId={panelId} client={client} />

      <SortHeader sort={panel.sort} panelId={panelId} client={client} />

      <div className="panel__list" ref={listRef}>
        {panel.entries.map((entry, i) => (
          <div
            key={entry.name}
            className={[
              "panel__row",
              i === panel.cursor ? "panel__row--cursor" : "",
              panel.selection.includes(i) ? "panel__row--selected" : "",
            ].filter(Boolean).join(" ")}
            onMouseDown={(e) => {
              if (e.button === 2) {
                const mode = panel.selection.includes(i) ? "deselect" : "select";
                rightDrag.current = { active: true, everMoved: false, startRow: i, mode, preSelection: [...panel.selection] };
                return;
              }
              if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
              client.sendIntent("SetCursor", "focused-panel", { index: i });
            }}
            onMouseEnter={() => {
              const drag = rightDrag.current;
              if (!drag.active) return;
              drag.everMoved = true;
              const lo = Math.min(drag.startRow, i);
              const hi = Math.max(drag.startRow, i);
              const rangeSet = new Set<number>();
              for (let r = lo; r <= hi; r++) rangeSet.add(r);
              let indices: number[];
              if (drag.mode === "deselect") {
                indices = drag.preSelection.filter(idx => !rangeSet.has(idx));
              } else {
                const merged = new Set(drag.preSelection);
                rangeSet.forEach(r => merged.add(r));
                indices = [...merged];
              }
              client.sendIntent("SetSelection", "focused-panel", { indices });
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              if (!rightDrag.current.everMoved) {
                client.sendIntent("ToggleSelectionAtIndex", "focused-panel", { index: i });
              }
            }}
            onDoubleClick={() => {
              client.sendIntent("SetCursor", "focused-panel", { index: i });
              if (entry.kind !== "directory") {
                client.sendIntent("OpenFile", "focused-panel");
              } else {
                client.sendIntent("OpenEntry", "focused-panel");
              }
            }}
          >
            <EntryIcon entry={entry} />
            <span className="panel__name">{entry.name}</span>
            <span className="panel__size">{formatSize(entry)}</span>
            <span className="panel__mtime">{formatMtime(entry.mtime)}</span>
            <span className="panel__perms">{entry.permissions ?? ""}</span>
          </div>
        ))}
        {panel.status === "loading" && panel.entries.length === 0 && (
          <div className="panel__loading">Loading…</div>
        )}
      </div>

      <div className="panel__footer">{statusText}</div>
    </div>
  );
}
