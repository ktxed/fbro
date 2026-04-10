import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import type { PanelState, VFSEntry, Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import {
  executeCommand,
  findMatchingCommand,
  GLOBAL_SHORTCUT_COMMAND_IDS,
  PANEL_SHORTCUT_COMMAND_IDS,
} from "../commands/interactive";
import { useAppDispatch, useAppState } from "../store";

const ROW_HEIGHT = 22;
const FILTER_DEBOUNCE_MS = 200;

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
  onRowPointerDown: (payload: { absIndex: number; clientX: number; clientY: number }) => void;
  dragActive: boolean;
  dragSource: boolean;
  dragHovered: boolean;
  dragDropAllowed: boolean;
  dragHoverAbsIndex: number | null;
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

export function Panel({
  panelId,
  panel,
  isFocused,
  modalActive,
  client,
  sources,
  otherPanelId,
  onRowPointerDown,
  dragActive,
  dragSource,
  dragHovered,
  dragDropAllowed,
  dragHoverAbsIndex,
}: PanelProps) {
  const appState = useAppState();
  const dispatch = useAppDispatch();
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const rightDrag = useRef({ active: false, everMoved: false, startRow: -1, mode: "select" as "select" | "deselect", preSelection: [] as number[] });
  const contextMenuOpenRef = useRef(false);
  const contextMenuHoldStartRef = useRef(0);
  const contextMenuPendingRef = useRef<null | {
    entry: VFSEntry;
    absIndex: number;
    rect: DOMRect;
  }>(null);
  const suppressNextContextMenuRef = useRef(false);
  const calcSet = new Set(panel.calculatingSize ?? []);
  const windowTimerRef = useRef<number>(0);

  const isWindowed = panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
  const totalCount = panel.totalCount ?? panel.entries.length;
  const windowStart = panel.windowStart ?? 0;
  const selectionSet = useMemo(() => new Set(panel.selection), [panel.selection]);

  const [filterInput, setFilterInput] = useState(panel.filter?.value ?? "");
  const filterInputRef = useRef<HTMLInputElement>(null);
  const filterDebounceRef = useRef<number>(0);
  const shouldFocusFilterRef = useRef(false);

  const sendFilterValue = useCallback((value: string) => {
    client.sendIntent("SetFilter", panelId, { value });
  }, [client, panelId]);

  const flushFilterValue = useCallback((value: string) => {
    clearTimeout(filterDebounceRef.current);
    if (value.trim() === (panel.filter?.value ?? "")) return;
    sendFilterValue(value);
  }, [panel.filter?.value, sendFilterValue]);

  const focusPanel = useCallback(() => {
    shouldFocusFilterRef.current = false;
    focusDivRef.current?.focus();
  }, []);

  const closeFilterMode = useCallback((options?: { clear?: boolean }) => {
    const clear = options?.clear ?? false;
    clearTimeout(filterDebounceRef.current);
    if (clear) {
      setFilterInput("");
      if ((panel.filter?.value ?? "") !== "" || filterInput.trim() !== "") {
        sendFilterValue("");
      }
    } else {
      flushFilterValue(filterInput);
    }
    shouldFocusFilterRef.current = false;
    client.sendIntent("SetFilterMode", panelId, { active: false });
    focusDivRef.current?.focus();
  }, [client, filterInput, flushFilterValue, panel.filter?.value, panelId, sendFilterValue]);

  const openFilterMode = useCallback((initialValue: string) => {
    clearTimeout(filterDebounceRef.current);
    shouldFocusFilterRef.current = true;
    setFilterInput(initialValue);
    client.sendIntent("SetFilterMode", panelId, { active: true });
    window.setTimeout(() => filterInputRef.current?.focus(), 0);
  }, [client, panelId]);

  // Keep focused panel's div focused so it receives keyboard events.
  // Also re-focus when a modal closes (modalActive transitions to false).
  useEffect(() => {
    if (!isFocused || modalActive) return;
    if (panel.filterModeActive && shouldFocusFilterRef.current) {
      window.setTimeout(() => filterInputRef.current?.focus(), 0);
    } else {
      focusDivRef.current?.focus();
    }
  }, [isFocused, modalActive, panel.filterModeActive]);

  useEffect(() => {
    if (!panel.filterModeActive) {
      shouldFocusFilterRef.current = false;
      setFilterInput(panel.filter?.value ?? "");
      return;
    }
    if (document.activeElement !== filterInputRef.current) {
      setFilterInput(panel.filter?.value ?? "");
    }
  }, [panel.filter?.value, panel.filterModeActive]);

  useEffect(() => {
    if (!panel.filterModeActive) {
      clearTimeout(filterDebounceRef.current);
      return;
    }
    if (filterInput.trim() === (panel.filter?.value ?? "")) {
      clearTimeout(filterDebounceRef.current);
      return;
    }
    clearTimeout(filterDebounceRef.current);
    filterDebounceRef.current = window.setTimeout(() => {
      sendFilterValue(filterInput);
    }, FILTER_DEBOUNCE_MS);
    return () => clearTimeout(filterDebounceRef.current);
  }, [filterInput, panel.filter?.value, panel.filterModeActive, sendFilterValue]);

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

  const openContextMenuForEntry = useCallback((entry: VFSEntry, absIndex: number, rect: DOMRect) => {
    const dirPath = panel.path.replace(/[/\\]+$/, "");
    const fullPath = entry.name === ".." ? dirPath : `${dirPath}/${entry.name}`;
    if (entry.name === "..") return;

    if (!isFocused) {
      client.sendIntent("SwitchFocus", undefined, { to: panelId });
    }
    client.sendIntent("SetCursor", panelId, { index: absIndex });
    rightDrag.current.active = false;
    contextMenuOpenRef.current = true;
    dispatch({
      type: "OPEN_CONTEXT_MENU",
      menu: {
        panelId,
        absIndex,
        name: entry.name,
        kind: entry.kind,
        size: entry.size,
        dirPath,
        fullPath,
        anchorLeft: rect.left,
        anchorRight: rect.right,
        anchorTop: rect.top,
        anchorBottom: rect.bottom,
      },
    });
  }, [client, dispatch, isFocused, panel.path, panelId]);

  const openContextMenuAtCursor = useCallback(() => {
    const entry = panel.entries[panel.cursor - (panel.windowStart ?? 0)];
    if (!entry || entry.name === "..") return;

    const row = listRef.current?.querySelector<HTMLElement>(`[data-abs-index="${panel.cursor}"]`);
    const rect = row?.getBoundingClientRect() ?? focusDivRef.current?.getBoundingClientRect();
    if (!rect) return;
    openContextMenuForEntry(entry, panel.cursor, rect);
  }, [openContextMenuForEntry, panel]);

  // End right-drag on mouse up anywhere; if the right button was held long
  // enough without becoming a drag, open the contextual menu on release.
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 2) {
        const heldLongEnough = contextMenuHoldStartRef.current > 0 && Date.now() - contextMenuHoldStartRef.current >= 400;
        const pending = contextMenuPendingRef.current;
        if (pending && heldLongEnough && !rightDrag.current.everMoved) {
          suppressNextContextMenuRef.current = true;
          openContextMenuForEntry(pending.entry, pending.absIndex, pending.rect);
        }
        rightDrag.current.active = false;
        contextMenuHoldStartRef.current = 0;
        contextMenuPendingRef.current = null;
      }
    };
    window.addEventListener("mouseup", onMouseUp);
    return () => window.removeEventListener("mouseup", onMouseUp);
  }, [openContextMenuForEntry]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (modalActive) {
      e.preventDefault();
      return;
    }
    if (panel.filterModeActive && e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeFilterMode({ clear: true });
      return;
    }
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      e.stopPropagation();
      openContextMenuAtCursor();
      return;
    }
    const command = findMatchingCommand(appState.commands, PANEL_SHORTCUT_COMMAND_IDS, e.nativeEvent);
    if (command) {
      e.preventDefault();
      e.stopPropagation();
      executeCommand(command.id, {
        state: appState,
        dispatch,
        client,
        panelId,
        panel,
        sources,
        otherPanelId,
        onOpenFilter: (initialValue) => {
          openFilterMode(initialValue);
        },
        getPanelPageSize: () => {
          const list = listRef.current;
          return list ? Math.max(1, Math.floor(list.clientHeight / ROW_HEIGHT)) : 1;
        },
      });
      return;
    }
  }

  const handleFilterKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      closeFilterMode();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeFilterMode({ clear: true });
      return;
    }
    const hasEditModifier = e.ctrlKey || e.metaKey;
    const isPrintable = e.key.length === 1 && !e.altKey && !hasEditModifier;
    const isEditShortcut = hasEditModifier && ["a", "c", "v", "x", "z", "y"].includes(e.key.toLowerCase());
    const isLocalEditingKey =
      isPrintable ||
      isEditShortcut ||
      e.key === "Backspace" ||
      e.key === "Delete" ||
      e.key === "ArrowLeft" ||
      e.key === "ArrowRight" ||
      e.key === "Home" ||
      e.key === "End";

    if (isLocalEditingKey) {
      e.stopPropagation();
      return;
    }

    const matchingPanelCommand = findMatchingCommand(appState.commands, PANEL_SHORTCUT_COMMAND_IDS, e.nativeEvent);
    if (matchingPanelCommand) {
      focusPanel();
      return;
    }

    const matchingGlobalCommand = findMatchingCommand(appState.commands, GLOBAL_SHORTCUT_COMMAND_IDS, e.nativeEvent);
    if (matchingGlobalCommand) {
      return;
    }

    e.stopPropagation();
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

  function handleClearSelection() {
    shouldFocusFilterRef.current = false;
    if (!isFocused && !modalActive) {
      client.sendIntent("SwitchFocus", undefined, { to: panelId });
    }
    client.sendIntent("ClearSelection", panelId);
    focusDivRef.current?.focus();
  }

  function renderRow(entry: VFSEntry, absIndex: number) {
    return (
      <div
        key={entry.name}
        data-abs-index={absIndex}
        className={[
          "panel__row",
          absIndex === panel.cursor ? "panel__row--cursor" : "",
          selectionSet.has(absIndex) ? "panel__row--selected" : "",
          dragActive && dragDropAllowed && dragHoverAbsIndex === absIndex ? "panel__row--drop-target" : "",
        ].filter(Boolean).join(" ")}
        style={isWindowed ? { position: "absolute", top: absIndex * ROW_HEIGHT, height: ROW_HEIGHT, left: 0, right: 0 } : undefined}
        onMouseDown={(e) => {
          if (e.button === 2) {
            const rect = DOMRect.fromRect({
              x: e.clientX,
              y: e.clientY,
              width: 0,
              height: 0,
            });
            if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
            const mode = selectionSet.has(absIndex) ? "deselect" : "select";
            rightDrag.current = { active: true, everMoved: false, startRow: absIndex, mode, preSelection: [...panel.selection] };
            contextMenuOpenRef.current = false;
            contextMenuHoldStartRef.current = Date.now();
            contextMenuPendingRef.current = entry.name === ".." ? null : { entry, absIndex, rect };
            return;
          }
          e.preventDefault();
          dispatch({ type: "CLOSE_CONTEXT_MENU" });
          if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
          onRowPointerDown({ absIndex, clientX: e.clientX, clientY: e.clientY });
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
          client.sendIntent("SetSelection", panelId, { indices });
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          if (contextMenuOpenRef.current) {
            window.setTimeout(() => {
              contextMenuOpenRef.current = false;
            }, 0);
            return;
          }
          if (suppressNextContextMenuRef.current) {
            suppressNextContextMenuRef.current = false;
            return;
          }
          if (!rightDrag.current.everMoved) {
            client.sendIntent("ToggleSelectionAtIndex", panelId, { index: absIndex });
          }
        }}
        onDoubleClick={() => {
          dispatch({ type: "CLOSE_CONTEXT_MENU" });
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
      className={[
        "panel",
        isFocused ? "panel--focused" : "",
        dragActive && dragSource ? "panel--drag-source" : "",
        dragActive && dragHovered ? "panel--drag-hover" : "",
        dragActive && dragDropAllowed ? "panel--drag-hover-valid" : "",
      ].filter(Boolean).join(" ")}
      data-panel-id={panelId}
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

      <VolumeHeader sources={sources} panel={panel} onNavigateToSource={(sourceId) => { shouldFocusFilterRef.current = false; client.sendIntent("NavigateToSource", panelId, { sourceId }); }} onMountSource={(sourceId) => { shouldFocusFilterRef.current = false; client.sendIntent("MountSource", panelId, { sourceId }); }} />

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

      {panel.filterModeActive && (
        <div className="panel__filter-bar">
          <input
            ref={filterInputRef}
            className="panel__filter-input"
            value={filterInput}
            onChange={e => setFilterInput(e.target.value)}
            onKeyDown={handleFilterKeyDown}
            onFocus={() => {
              shouldFocusFilterRef.current = true;
            }}
            placeholder="filter…"
            spellCheck={false}
          />
        </div>
      )}

      <div className="panel__footer">
        <div className="panel__footer-left">
          {selCount > 0 && (
            <button
              className="panel__selection-clear"
              tabIndex={-1}
              type="button"
              title="Clear selection"
              aria-label="Clear selection"
              onMouseDown={(e) => e.preventDefault()}
              onClick={handleClearSelection}
            >
              ×
            </button>
          )}
          <span className="panel__footer-status">{statusText}</span>
        </div>
        <div className="panel__footer-right">
          {panel.filter && <span className="panel__filter-active">/{panel.filter.value}/</span>}
          {diskText && <span>{diskText}</span>}
        </div>
      </div>
    </div>
  );
}
