import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import type { PanelState, VFSEntry, Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import { modKey } from "../utils/platform";
import { useAppState } from "../store";

const ROW_HEIGHT = 22;

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
  otherPanelId: string;
}

function VolumeHeader({ sources, panel, onNavigateToSource, onMountSource }: {
  sources: Record<string, Source>;
  panel: PanelState;
  onNavigateToSource: (sourceId: string) => void;
  onMountSource: (sourceId: string) => void;
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

  const onWheel = (e: React.WheelEvent) => {
    const el = listRef.current;
    if (!el || el.scrollWidth <= el.clientWidth) return;
    e.preventDefault();
    el.scrollBy({ left: e.deltaY !== 0 ? e.deltaY : e.deltaX, behavior: "smooth" });
  };

  return (
    <div className="panel__volumes">
      <button className="panel__volumes-arrow" disabled={!canLeft} tabIndex={-1} onClick={() => scroll(-1)}>‹</button>
      <div className="panel__volumes-list" ref={listRef} onScroll={updateArrows} onWheel={onWheel}>
        {Object.values(sources).map(src => (
          <button
            key={src.id}
            className={`panel__volume-btn${src.id === panel.sourceId ? " panel__volume-btn--active" : ""}${!src.mounted ? " panel__volume-btn--unmounted" : ""}`}
            tabIndex={-1}
            onClick={() => src.mounted ? onNavigateToSource(src.id) : onMountSource(src.id)}
            title={src.mounted ? src.rootPath : `${src.name} (unmounted — click to mount)`}
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
  const { fileServerPort } = useAppState();
  const iconName = entry.iconName;
  return (
    <span className="panel__col-icon">
      {iconName && !failed && fileServerPort > 0 ? (
        <img
          src={`http://127.0.0.1:${fileServerPort}/__icon__/${iconName}`}
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

function formatBytes(s: number): string {
  if (s < 1024) return `${s}`;
  if (s < 1024 * 1024) return `${(s / 1024).toFixed(1)}K`;
  if (s < 1024 * 1024 * 1024) return `${(s / (1024 * 1024)).toFixed(1)}M`;
  return `${(s / (1024 * 1024 * 1024)).toFixed(1)}G`;
}

function formatSize(entry: VFSEntry, calcSet: ReadonlySet<string>): string {
  if (entry.kind === "directory") {
    if (calcSet.has(entry.name)) return "⏳";
    if (entry.size !== undefined) return formatBytes(entry.size);
    return "<DIR>";
  }
  if (entry.size === undefined) return "";
  return formatBytes(entry.size);
}

function formatMtime(mtime: string | undefined): string {
  if (!mtime) return "";
  const secs = parseInt(mtime, 10);
  if (isNaN(secs)) return "";
  const d = new Date(secs * 1000);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function Panel({ panelId, panel, isFocused, modalActive, client, sources, otherPanelId }: PanelProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const rightDrag = useRef({ active: false, everMoved: false, startRow: -1, mode: "select" as "select" | "deselect", preSelection: [] as number[] });
  const calcSet = new Set(panel.calculatingSize ?? []);
  const windowTimerRef = useRef<number>(0);

  const isWindowed = panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
  const totalCount = panel.totalCount ?? panel.entries.length;
  const windowStart = panel.windowStart ?? 0;
  const selectionSet = useMemo(() => new Set(panel.selection), [panel.selection]);

  const [filterOpen, setFilterOpen] = useState(false);
  const [filterInput, setFilterInput] = useState("");
  const filterInputRef = useRef<HTMLInputElement>(null);

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
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;

    if (isWindowed) {
      // In windowed mode, use absolute cursor position to scroll
      const cursorTop = panel.cursor * ROW_HEIGHT;
      const cursorBottom = cursorTop + ROW_HEIGHT;
      if (cursorTop < list.scrollTop) {
        list.scrollTop = cursorTop;
      } else if (cursorBottom > list.scrollTop + list.clientHeight) {
        list.scrollTop = cursorBottom - list.clientHeight;
      }
    } else {
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
    }
  }, [panel.cursor, isWindowed]);

  // Debounced window request on scroll (Mode B only)
  const handleScroll = useCallback(() => {
    if (!isWindowed) return;
    clearTimeout(windowTimerRef.current);
    windowTimerRef.current = window.setTimeout(() => {
      const list = listRef.current;
      if (!list) return;
      const scrollTop = list.scrollTop;
      const viewportHeight = list.clientHeight;
      const firstVisible = Math.floor(scrollTop / ROW_HEIGHT);
      const visibleCount = Math.ceil(viewportHeight / ROW_HEIGHT);
      const buffer = visibleCount; // 1 screen buffer on each side
      const start = Math.max(0, firstVisible - buffer);
      const count = visibleCount + 2 * buffer;
      client.sendIntent("RequestWindow", panelId, { start, count });
    }, 50);
  }, [isWindowed, panelId, client]);

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
      case " ": {
        e.preventDefault();
        if (e.shiftKey) {
          // Shift+Space: calculate sizes for all folders in the panel, or only
          // selected folders if the selection contains any.
          const entriesToCalc = panel.selection.length > 0
            ? panel.selection.map(i => isWindowed
                ? (i >= windowStart && i < windowStart + panel.entries.length ? panel.entries[i - windowStart] : undefined)
                : panel.entries[i])
            : panel.entries;
          for (const entry of entriesToCalc) {
            if (!entry || entry.kind !== "directory") continue;
            if (calcSet.has(entry.name)) {
              client.sendIntent("CancelSizeCalculation", "focused-panel", { name: entry.name });
            } else {
              client.sendIntent("CalculateSize", "focused-panel", { name: entry.name });
            }
          }
        } else {
          // Space: if cursor is on a folder with no selection, toggle its size
          // calculation; otherwise toggle selection.
          const cursorEntry = isWindowed
            ? (panel.cursor >= windowStart && panel.cursor < windowStart + panel.entries.length
                ? panel.entries[panel.cursor - windowStart] : undefined)
            : panel.entries[panel.cursor];
          if (cursorEntry?.kind === "directory" && panel.selection.length === 0) {
            if (calcSet.has(cursorEntry.name)) {
              client.sendIntent("CancelSizeCalculation", "focused-panel", { name: cursorEntry.name });
            } else {
              client.sendIntent("CalculateSize", "focused-panel", { name: cursorEntry.name });
            }
          } else {
            client.sendIntent("ToggleSelection", "focused-panel");
          }
        }
        break;
      }
      case "a":
      case "A":
        if (modKey(e)) {
          e.preventDefault();
          if (e.shiftKey) {
            client.sendIntent("ClearSelection", "focused-panel");
          } else {
            client.sendIntent("SelectAll", "focused-panel");
          }
        }
        break;
      case "i":
        if (modKey(e)) {
          e.preventDefault();
          client.sendIntent("InvertSelection", "focused-panel");
        }
        break;
      case "PageUp":
      case "PageDown": {
        e.preventDefault();
        const list = listRef.current;
        if (!list) break;
        const pageSize = Math.max(1, Math.floor(list.clientHeight / ROW_HEIGHT));
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
        // In windowed mode, the BE knows the cursor entry — just send OpenEntry/OpenFile
        if (isWindowed) {
          const localIdx = panel.cursor - windowStart;
          const entry = panel.entries[localIdx];
          if (entry && entry.kind !== "directory") {
            client.sendIntent("OpenFile", "focused-panel");
          } else {
            client.sendIntent("OpenEntry", "focused-panel");
          }
        } else {
          const entry = panel.entries[panel.cursor];
          if (entry && entry.kind !== "directory") {
            client.sendIntent("OpenFile", "focused-panel");
          } else {
            client.sendIntent("OpenEntry", "focused-panel");
          }
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
      case "F9":
        e.preventDefault();
        client.sendIntent("OpenTerminal", "focused-panel");
        break;
      case "Tab":
        e.preventDefault();
        if (e.ctrlKey || e.shiftKey) {
          // Ctrl+Tab: next source, Shift+Tab / Ctrl+Shift+Tab: previous source
          const sourceList = Object.values(sources);
          if (sourceList.length > 1) {
            const cur = sourceList.findIndex(s => s.id === panel.sourceId);
            const next = e.shiftKey
              ? (cur - 1 + sourceList.length) % sourceList.length
              : (cur + 1) % sourceList.length;
            client.sendIntent("NavigateToSource", panelId, { sourceId: sourceList[next].id });
            setFilterOpen(false);
          }
        } else {
          client.sendIntent("SwitchFocus", undefined, {
            to: panelId === "left" ? "right" : "left",
          });
        }
        break;
      case "f":
        if (!modKey(e)) {
          e.preventDefault();
          setFilterInput(panel.filter?.value ?? "");
          setFilterOpen(true);
          setTimeout(() => filterInputRef.current?.focus(), 0);
        }
        break;
      case "s":
        if (!modKey(e) && otherPanelId) {
          e.preventDefault();
          client.sendIntent("NavigatePanelTo", undefined, {
            panelId: otherPanelId,
            path: panel.path,
          });
        }
        break;
    }
  }

  const closeFilter = () => {
    setFilterOpen(false);
    focusDivRef.current?.focus();
  };

  const handleFilterKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    e.stopPropagation();
    if (e.key === "Enter") {
      client.sendIntent("SetFilter", panelId, { value: filterInput });
      closeFilter();
    } else if (e.key === "Escape") {
      closeFilter();
    }
  };

  const itemCount = isWindowed ? totalCount : panel.entries.length;
  // In non-windowed mode recompute total from live entries so calculated dir sizes are included.
  const totalSize = !isWindowed
    ? panel.entries.reduce((sum, e) => e.size !== undefined ? sum + e.size : sum, 0)
    : panel.totalSize;
  const selCount = panel.selection.length;
  // In windowed mode entries outside the window aren't available on the FE, so skip sel size.
  const selSize = !isWindowed && selCount > 0
    ? panel.selection.reduce((sum, i) => {
        const e = panel.entries[i];
        return e && e.size !== undefined ? sum + e.size : sum;
      }, 0)
    : undefined;
  const statusText =
    panel.status === "loading"
      ? "Loading…"
      : selCount > 0
        ? `${selCount} / ${itemCount} item${itemCount !== 1 ? "s" : ""}${selSize !== undefined && totalSize !== undefined && (selSize > 0 || totalSize > 0) ? ` (${formatBytes(selSize)} / ${formatBytes(totalSize)})` : ""}`
        : `${itemCount} item${itemCount !== 1 ? "s" : ""}${totalSize !== undefined && totalSize > 0 ? ` (${formatBytes(totalSize)})` : ""}`;

  const source = sources[panel.sourceId];
  const diskText = (source?.freeBytes !== undefined && source?.totalBytes !== undefined)
    ? `${formatBytes(source.freeBytes)} / ${formatBytes(source.totalBytes)} free`
    : "";

  function renderRow(entry: VFSEntry, absIndex: number) {
    return (
      <div
        key={entry.name}
        className={[
          "panel__row",
          absIndex === panel.cursor ? "panel__row--cursor" : "",
          selectionSet.has(absIndex) ? "panel__row--selected" : "",
        ].filter(Boolean).join(" ")}
        style={isWindowed ? { position: "absolute", top: absIndex * ROW_HEIGHT, height: ROW_HEIGHT, left: 0, right: 0 } : undefined}
        onMouseDown={(e) => {
          if (e.button === 2) {
            const mode = selectionSet.has(absIndex) ? "deselect" : "select";
            rightDrag.current = { active: true, everMoved: false, startRow: absIndex, mode, preSelection: [...panel.selection] };
            return;
          }
          if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
          client.sendIntent("SetCursor", "focused-panel", { index: absIndex });
        }}
        onMouseEnter={() => {
          const drag = rightDrag.current;
          if (!drag.active) return;
          drag.everMoved = true;
          const lo = Math.min(drag.startRow, absIndex);
          const hi = Math.max(drag.startRow, absIndex);
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
            client.sendIntent("ToggleSelectionAtIndex", "focused-panel", { index: absIndex });
          }
        }}
        onDoubleClick={() => {
          client.sendIntent("SetCursor", "focused-panel", { index: absIndex });
          if (entry.kind !== "directory") {
            client.sendIntent("OpenFile", "focused-panel");
          } else {
            client.sendIntent("OpenEntry", "focused-panel");
          }
        }}
      >
        <EntryIcon entry={entry} />
        <span className="panel__name">{entry.name}</span>
        <span className="panel__size">{formatSize(entry, calcSet)}</span>
        <span className="panel__mtime">{formatMtime(entry.mtime)}</span>
        <span className="panel__perms">{entry.permissions ?? ""}</span>
      </div>
    );
  }

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
          title="Navigate to source root"
          onClick={() => client.sendIntent("NavigateSourceRoot", panelId)}
        >/</button>
        <button
          className="panel__home-btn"
          tabIndex={-1}
          title="Navigate to home folder"
          onClick={() => client.sendIntent("NavigateHome", panelId)}
        >~</button>
      </div>

      <VolumeHeader sources={sources} panel={panel} onNavigateToSource={(sourceId) => { client.sendIntent("NavigateToSource", panelId, { sourceId }); setFilterOpen(false); }} onMountSource={(sourceId) => { client.sendIntent("MountSource", panelId, { sourceId }); }} />

      <SortHeader sort={panel.sort} panelId={panelId} client={client} />

      <div className="panel__list" ref={listRef} onScroll={isWindowed ? handleScroll : undefined}>
        {isWindowed ? (
          <div className="panel__virtual-sizer" style={{ height: totalCount * ROW_HEIGHT, position: "relative" }}>
            {panel.entries.map((entry, i) => renderRow(entry, windowStart + i))}
          </div>
        ) : (
          <>
            {panel.entries.map((entry, i) => renderRow(entry, i))}
            {panel.status === "loading" && panel.entries.length === 0 && (
              <div className="panel__loading">Loading…</div>
            )}
          </>
        )}
      </div>

      {filterOpen && (
        <div className="panel__filter-bar">
          <input
            ref={filterInputRef}
            className="panel__filter-input"
            value={filterInput}
            onChange={e => setFilterInput(e.target.value)}
            onKeyDown={handleFilterKeyDown}
            placeholder="filter…"
            spellCheck={false}
          />
        </div>
      )}

      <div className="panel__footer">
        <span>{statusText}</span>
        {panel.filter && <span className="panel__filter-active">/{panel.filter.value}/</span>}
        {diskText && <span>{diskText}</span>}
      </div>
    </div>
  );
}
