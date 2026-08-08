import { useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from "react";
import { Tooltip } from "react-tooltip";
import type { PanelState, VFSEntry, Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import {
  executeCommand,
  findMatchingCommand,
  GLOBAL_SHORTCUT_COMMAND_IDS,
  PANEL_SHORTCUT_COMMAND_IDS,
} from "../commands/interactive";
import { useAppDispatch, useAppState } from "../store";
import { IconChevronDown, IconChevronUp, IconChevronRight, IconChevronLeft, IconArrowUpRight, IconX, IconAlertTriangle, IconActivity, IconHistory, IconGitBranch, IconGitCompare, IconLock, IconMinus, IconNetwork, IconListCheck, IconPlus, IconRestore } from "@tabler/icons-react";

// Maps Source.iconName (BE-provided) to a Tabler icon component for rendering
// in the volume bar. Sources with an iconName render the icon in place of
// their text label; unknown names fall back to the text label.
const SOURCE_ICONS: Record<string, React.ComponentType<{ size?: number }>> = {
  activity: IconActivity,
  history: IconHistory,
  "list-check": IconListCheck,
};
import { BookmarksPopover } from "./BookmarksPopover";
import { PanelTabBar } from "./PanelTabBar";
import { PanelTerminal } from "./PanelTerminal";
import { EntryThumb } from "./EntryThumb";
import { isGlobalSelectionSourceId } from "../utils/sourceId";
import { GitModal } from "./GitModal";
import { GitActionsModal } from "./GitActionsModal";
import type { GitActivity } from "./gitActivity";

const ROW_HEIGHT = 22;
const GRID_THUMB_SIZE = 128;
const GRID_CARD_MIN_WIDTH = 144;
const GRID_GAP = 8;
const GRID_PADDING = 16; // 8px on each side from .panel__list--grid padding
const FILTER_DEBOUNCE_MS = 200;
const SORT_COLUMNS = [
  { id: "kind", label: "Kind" },
  { id: "name", label: "Name" },
  { id: "size", label: "Size" },
  { id: "date", label: "Date" },
] as const;

function GitBubbleActions({
  panelId,
  path,
  label,
  staged,
  client,
  onExitKeyboardMode,
  onRestoreStageFocus,
}: {
  panelId: string;
  path: string;
  label: string;
  staged: boolean;
  client: WsClient;
  onExitKeyboardMode: () => void;
  onRestoreStageFocus: () => void;
}) {
  const [rollbackArmed, setRollbackArmed] = useState(false);
  const tooltipId = `git-inline-actions-${panelId}`;

  useEffect(() => {
    setRollbackArmed(false);
  }, [path, staged]);

  const runPathAction = (name: "GitStagePaths" | "GitUnstagePaths" | "GitRollbackPaths") => {
    setRollbackArmed(false);
    client.sendIntent(name, undefined, { panelId, paths: [path] });
  };
  const runStageAction = () => {
    onRestoreStageFocus();
    runPathAction(staged ? "GitUnstagePaths" : "GitStagePaths");
  };
  const runRollbackAction = () => {
    if (rollbackArmed) {
      onExitKeyboardMode();
      runPathAction("GitRollbackPaths");
    }
    else setRollbackArmed(true);
  };
  const stopRowInteraction = (event: React.SyntheticEvent) => event.stopPropagation();
  const handleToolbarKeyDown = (event: React.KeyboardEvent<HTMLSpanElement>) => {
    const unmodified = !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey;
    if (unmodified && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button")];
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (buttons.length > 0 && current >= 0) {
        event.preventDefault();
        event.stopPropagation();
        const direction = event.key === "ArrowRight" ? 1 : -1;
        buttons[(current + direction + buttons.length) % buttons.length].focus();
      }
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.stopPropagation();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onExitKeyboardMode();
      return;
    }
    if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      onExitKeyboardMode();
    }
  };

  return (
    <span className="panel__git-bubble panel__git-bubble--actions" data-testid="git-inline-bubble">
      <span className="panel__git-bubble-label">{label}</span>
      <span className="panel__git-bubble-buttons" role="toolbar" aria-label={`Git actions for ${path}`} onKeyDown={handleToolbarKeyDown}>
        <button
          type="button"
          tabIndex={-1}
          className="panel__git-bubble-action"
          data-testid="git-inline-view-diff"
          data-tooltip-id={tooltipId}
          data-tooltip-content="View Git diff"
          aria-label={`View Git diff for ${path}`}
          onMouseDown={stopRowInteraction}
          onDoubleClick={stopRowInteraction}
          onClick={(event) => {
            event.stopPropagation();
            setRollbackArmed(false);
            client.sendIntent("GitViewDiff", undefined, { panelId, targetPath: path });
          }}
        >
          <IconGitCompare size={13} aria-hidden />
        </button>
        <button
          type="button"
          tabIndex={-1}
          className="panel__git-bubble-action"
          data-testid={staged ? "git-inline-unstage" : "git-inline-stage"}
          data-tooltip-id={tooltipId}
          data-tooltip-content={staged ? "Unstage" : "Stage"}
          aria-label={`${staged ? "Unstage" : "Stage"} ${path}`}
          onMouseDown={stopRowInteraction}
          onDoubleClick={stopRowInteraction}
          onClick={(event) => { event.stopPropagation(); runStageAction(); }}
        >
          {staged ? <IconMinus size={13} aria-hidden /> : <IconPlus size={13} aria-hidden />}
        </button>
        <button
          type="button"
          tabIndex={-1}
          className={`panel__git-bubble-action panel__git-bubble-action--danger${rollbackArmed ? " panel__git-bubble-action--armed" : ""}`}
          data-testid="git-inline-rollback"
          data-tooltip-id={tooltipId}
          data-tooltip-content={rollbackArmed ? "Confirm rollback" : "Rollback"}
          aria-label={`${rollbackArmed ? "Confirm rollback" : "Rollback"} ${path}`}
          onMouseDown={stopRowInteraction}
          onDoubleClick={stopRowInteraction}
          onClick={(event) => { event.stopPropagation(); runRollbackAction(); }}
        >
          {rollbackArmed ? <IconAlertTriangle size={13} aria-hidden /> : <IconRestore size={13} aria-hidden />}
        </button>
      </span>
    </span>
  );
}

function computeGridMetrics(listEl: HTMLElement | null) {
  if (!listEl) return { cols: 1, cardWidth: GRID_CARD_MIN_WIDTH, cardHeight: 168, pageRows: 1 };
  // A tab or layout can be briefly hidden while it is being swapped. Avoid
  // turning that transient measurement into invalid absolute card positions.
  const width = listEl.getBoundingClientRect().width || listEl.clientWidth;
  const inner = Math.max(GRID_CARD_MIN_WIDTH, Number.isFinite(width) ? width - GRID_PADDING : GRID_CARD_MIN_WIDTH);
  const cols = Math.max(1, Math.floor((inner + GRID_GAP) / (GRID_CARD_MIN_WIDTH + GRID_GAP)));
  const cardWidth = Math.floor((inner - (cols - 1) * GRID_GAP) / cols);
  // Card height = thumb (4:3 from card width)
  //               + name (~16px, 1 line)
  //               + size (~13px) + date (~13px) on separate lines
  //               + 4px card padding × 2 + a few px slack
  // Total non-thumb chrome ≈ 52 px.
  const cardHeight = Math.round(cardWidth * 0.75) + 52;
  const pageRows = Math.max(1, Math.floor(listEl.clientHeight / (cardHeight + GRID_GAP)));
  return { cols, cardWidth, cardHeight, pageRows };
}

function colorClassForEntry(entry: VFSEntry): string | undefined {
  if (entry.name.startsWith(".") && entry.name !== "..") return "cc-dot";
  return entry.cc;
}

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
    const Icon = active && sort.order === "desc" ? IconChevronDown : IconChevronUp;
    return <span className={`panel__sort-tri${active ? " panel__sort-tri--active" : ""}`}><Icon size={9} /></span>;
  }

  return (
    <div className="panel__sort-header">
      <span className="panel__sort-kind" onClick={() => handleSort("kind")}>{tri("kind")}</span>
      <span className="panel__sort-name" onClick={() => handleSort("name")}>Name {tri("name")}</span>
      <span className="panel__sort-size" onClick={() => handleSort("size")}>Size {tri("size")}</span>
      <span className="panel__sort-date" onClick={() => handleSort("date")}>Date {tri("date")}</span>
    </div>
  );
}

function SortSelector({
  draft,
  current,
}: {
  draft: PanelState["sort"];
  current: PanelState["sort"];
}) {
  return (
    <div className="panel__sort-selector" data-testid="panel-sort-selector">
      <div className="panel__sort-selector-row">
        {SORT_COLUMNS.map((column) => {
          const active = draft.by === column.id;
          const applied = current.by === column.id;
          const Icon = draft.order === "desc" ? IconChevronDown : IconChevronUp;
          return (
            <div
              key={column.id}
              className={[
                "panel__sort-selector-col",
                active ? "panel__sort-selector-col--active" : "",
                applied ? "panel__sort-selector-col--applied" : "",
              ].filter(Boolean).join(" ")}
              data-sort-column={column.id}
              data-sort-active={active ? "true" : "false"}
            >
              <span className="panel__sort-selector-label">{column.label}</span>
              <span className="panel__sort-selector-icon">
                {active ? <Icon size={14} /> : applied ? (current.order === "desc" ? <IconChevronDown size={13} /> : <IconChevronUp size={13} />) : null}
              </span>
            </div>
          );
        })}
      </div>
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
  externalDragHovered: boolean;
}

function VolumeHeader({ sources, panel, onNavigateToSource, onMountSource, client }: {
  sources: Record<string, Source>;
  panel: PanelState;
  onNavigateToSource: (sourceId: string) => void;
  onMountSource: (sourceId: string) => void;
  client: WsClient;
}) {
  const remoteStatus = useAppState().remoteSourceStatus;
  const listRef = useRef<HTMLDivElement>(null);
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);
  const [expandedRemoteIds, setExpandedRemoteIds] = useState<Set<string>>(() => new Set());
  const [overflowingRemoteIds, setOverflowingRemoteIds] = useState<Set<string>>(() => new Set());

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

  const updateRemoteOverflow = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    setOverflowingRemoteIds((current) => {
      const next = new Set<string>();
      el.querySelectorAll<HTMLElement>(".panel__remote-volume").forEach((node) => {
        const sourceId = node.dataset.sourceId;
        const label = node.querySelector<HTMLElement>(".panel__volume-btn-label");
        if (!sourceId || !label) return;
        if (expandedRemoteIds.has(sourceId)) {
          if (current.has(sourceId)) next.add(sourceId);
          return;
        }
        if (label.scrollWidth > label.clientWidth + 1) {
          next.add(sourceId);
        }
      });
      if (next.size === current.size && [...next].every((id) => current.has(id))) {
        return current;
      }
      return next;
    });
  }, [expandedRemoteIds]);

  useLayoutEffect(() => {
    updateRemoteOverflow();
    const onResize = () => updateRemoteOverflow();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [sources, expandedRemoteIds, updateRemoteOverflow]);

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
      <button className="panel__volumes-arrow" disabled={!canLeft} tabIndex={-1} onClick={() => scroll(-1)}><IconChevronLeft size={14} /></button>
      <div className="panel__volumes-list" ref={listRef} onScroll={updateArrows} onWheel={onWheel}>
        {Object.values(sources).filter(src => !src.shadow).map(src => {
          const Icon = src.iconName ? SOURCE_ICONS[src.iconName] : undefined;
          const isVirtual = src.kind === "virtual";
          const isRemote = src.kind === "remote";
          const status = isRemote ? remoteStatus[src.id] : undefined;
          // The reducer drops the entry when "connected" — anything we
          // see here means the supervisor is unhappy.
          const isDegraded = !!status;
          const classes = [
            "panel__volume-btn",
            src.id === panel.sourceId ? "panel__volume-btn--active" : "",
            !src.mounted ? "panel__volume-btn--unmounted" : "",
            Icon ? "panel__volume-btn--icon-only" : "",
            isVirtual ? "panel__volume-btn--virtual" : "",
            isRemote ? "panel__volume-btn--remote" : "",
            isDegraded ? `panel__volume-btn--${status!.status}` : "",
          ].filter(Boolean).join(" ");
          const baseTitle = src.mounted
            ? isRemote && src.peer
              ? `Remote source:\n\t${src.name}\nPeer: \n\t${src.peer.name}${src.peer.os ? ` (${src.peer.os})` : ""}\nAccess: read-only`
              : (Icon ? src.name : src.rootPath)
            : `${src.name} (unmounted — click to mount)`;
          const title = isDegraded
            ? `${baseTitle}\n${status!.status}${status!.message ? `: ${status!.message}` : ""}\nClick to reconnect now.`
            : baseTitle;
          const onClick = () => {
            if (isDegraded) {
              client.sendIntent("ReconnectRemoteSource", undefined, { sourceId: src.id });
              return;
            }
            if (src.mounted) onNavigateToSource(src.id);
            else onMountSource(src.id);
          };
          if (isRemote) {
            const expanded = expandedRemoteIds.has(src.id);
            const canExpand = overflowingRemoteIds.has(src.id);
            const toggleExpanded = () => {
              setExpandedRemoteIds((current) => {
                const next = new Set(current);
                if (next.has(src.id)) next.delete(src.id);
                else next.add(src.id);
                return next;
              });
            };
            return (
              <span
                key={src.id}
                data-source-id={src.id}
                className={`panel__remote-volume${src.id === panel.sourceId ? " panel__remote-volume--active" : ""}${canExpand ? " panel__remote-volume--expandable" : ""}${expanded ? " panel__remote-volume--expanded" : ""}${isDegraded ? ` panel__remote-volume--${status!.status}` : ""}`}
                title={title}
              >
                <button
                  className={classes}
                  tabIndex={-1}
                  onClick={onClick}
                  title={title}
                >
                  <IconNetwork size={13} className="panel__volume-btn-net" aria-hidden />
                  {!isDegraded && <IconLock size={12} className="panel__volume-btn-lock" aria-hidden />}
                  <span className="panel__volume-btn-label">{src.name}</span>
                  {isDegraded && (
                    <span className="panel__volume-btn-spinner" aria-hidden title="Reconnecting — click to retry">
                      ⟳
                    </span>
                  )}
                </button>
                {canExpand && (
                  <button
                    className="panel__remote-volume-toggle"
                    tabIndex={-1}
                    onClick={toggleExpanded}
                    title={expanded ? "Collapse remote volume label" : "Expand remote volume label"}
                  >
                    {expanded ? "<<" : ">>"}
                  </button>
                )}
              </span>
            );
          }
          return (
            <button
              key={src.id}
              className={classes}
              tabIndex={-1}
              onClick={onClick}
              title={title}
            >
              {Icon ? <Icon size={14} /> : src.name}
            </button>
          );
        })}
      </div>
      <button className="panel__volumes-arrow" disabled={!canRight} tabIndex={-1} onClick={() => scroll(1)}><IconChevronRight size={14} /></button>
    </div>
  );
}

function buildBreadcrumbs(path: string): Array<{ label: string; path: string }> {
  if (!path) return [];

  const normalized = path.replace(/\\/g, "/");
  if (normalized === "/") {
    return [{ label: "/", path: "/" }];
  }

  const isAbsolute = normalized.startsWith("/");
  const parts = normalized.split("/").filter(Boolean);
  const crumbs: Array<{ label: string; path: string }> = [];

  if (isAbsolute) {
    crumbs.push({ label: "/", path: "/" });
  }

  let currentPath = isAbsolute ? "" : "";
  for (const part of parts) {
    currentPath = isAbsolute
      ? `${currentPath}/${part}`
      : currentPath
        ? `${currentPath}/${part}`
        : part;
    // Windows drive root: "C:" alone is relative (CWD on that drive); "C:/" is absolute.
    const crumbPath = /^[A-Za-z]:$/.test(currentPath) ? `${currentPath}/` : (currentPath || "/");
    crumbs.push({ label: part, path: crumbPath });
  }

  return crumbs;
}

function PathBreadcrumbs({ panelId, path, client, shadowSource }: {
  panelId: string;
  path: string;
  client: WsClient;
  shadowSource?: import("../ws/types").ShadowSource;
}) {
  const listRef = useRef<HTMLDivElement>(null);

  // When inside an archive, build breadcrumbs for the archive's host path first,
  // then append the internal archive path crumbs.
  const archiveCrumbs = useMemo(() => shadowSource ? buildBreadcrumbs(shadowSource.archivePath) : [], [shadowSource]);
  const innerCrumbs = useMemo(() => {
    if (!shadowSource) return buildBreadcrumbs(path);
    if (!path) return [];
    // Build relative crumbs for the path inside the archive.
    const parts = path.split("/").filter(Boolean);
    const result: Array<{ label: string; path: string }> = [];
    let current = "";
    for (const part of parts) {
      current = current ? `${current}/${part}` : part;
      result.push({ label: part, path: current });
    }
    return result;
  }, [path, shadowSource]);

  const allCrumbs = shadowSource ? [...archiveCrumbs, ...innerCrumbs] : buildBreadcrumbs(path);
  const archiveBoundary = shadowSource ? archiveCrumbs.length : -1;

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const id = window.requestAnimationFrame(() => {
      el.scrollLeft = el.scrollWidth;
    });
    return () => window.cancelAnimationFrame(id);
  }, [path, allCrumbs.length]);

  const handleWheel = useCallback((e: React.WheelEvent<HTMLDivElement>) => {
    const el = listRef.current;
    if (!el || el.scrollWidth <= el.clientWidth) return;
    if (Math.abs(e.deltaX) < 0.5 && Math.abs(e.deltaY) < 0.5) return;
    e.preventDefault();
    el.scrollBy({ left: e.deltaX !== 0 ? e.deltaX : e.deltaY, behavior: "auto" });
  }, []);

  const fullTitle = shadowSource ? `${shadowSource.archivePath} :: ${path || "/"}` : path;

  return (
    <div className="panel__path" title={fullTitle}>
      <div className="panel__path-scroll" ref={listRef} onWheel={handleWheel}>
        {allCrumbs.map((crumb, index) => {
          const isArchiveSegment = index < archiveBoundary;
          const isArchiveFile = index === archiveBoundary - 1;
          const isInner = shadowSource && index >= archiveBoundary;
          const isLast = index === allCrumbs.length - 1;

          const colorClass = (isArchiveFile || isInner) ? " panel__breadcrumb-btn--archive" : "";

          return (
            <span className="panel__breadcrumb" key={`${crumb.path}-${index}`}>
              {index > 0 && allCrumbs[index - 1]?.path !== "/" && (
                <span className={`panel__breadcrumb-sep${isArchiveFile ? " panel__breadcrumb-sep--archive" : ""}`}>
                  {isArchiveFile ? " :: " : "/"}
                </span>
              )}
              <button
                className={`panel__breadcrumb-btn${isLast ? " panel__breadcrumb-btn--active" : ""}${colorClass}`}
                type="button"
                tabIndex={-1}
                onClick={() => {
                  if (isArchiveSegment && !isArchiveFile) {
                    // Clicking a host-path crumb while in archive: navigate up and out.
                    client.sendIntent("NavigateHome", "focused-panel");
                  } else if (isArchiveFile && !isLast) {
                    // Clicking the archive root crumb while inside: navigate to archive root.
                    client.sendIntent("NavigatePanelTo", panelId, { panelId, path: "" });
                  } else if (isInner || (shadowSource && isLast && index >= archiveBoundary)) {
                    // Navigate within the archive.
                    client.sendIntent("NavigatePanelTo", panelId, { panelId, path: crumb.path });
                  } else if (!shadowSource) {
                    client.sendIntent("NavigatePanelTo", panelId, { panelId, path: crumb.path });
                  }
                }}
                title={crumb.path}
              >
                {crumb.label}
              </button>
            </span>
          );
        })}
      </div>
    </div>
  );
}

function fallbackIcon(entry: VFSEntry): React.ReactNode {
  switch (entry.kind) {
    case "directory": return <IconChevronRight size={14} />;
    case "symlink":   return <IconArrowUpRight size={14} />;
    case "file":      return null;
    default:          return <span>?</span>;
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
        fallbackIcon(entry)
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

function formatSize(entry: VFSEntry, calcSet: ReadonlySet<string>, directorySizeAsCount = false): string {
  if (entry.kind === "directory") {
    if (calcSet.has(entry.name)) return "⏳";
    if (directorySizeAsCount && entry.size !== undefined) {
      return `${entry.size} item${entry.size === 1 ? "" : "s"}`;
    }
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

function splitNameExt(entry: VFSEntry): { base: string; ext: string } {
  const n = entry.name;
  if (entry.kind === "directory" || n === ".." || n.startsWith(".")) {
    return { base: n, ext: "" };
  }
  const i = n.lastIndexOf(".");
  if (i <= 0 || i === n.length - 1) return { base: n, ext: "" };
  return { base: n.slice(0, i), ext: n.slice(i) };
}

function isRestricted(entry: VFSEntry): boolean {
  const p = entry.permissions;
  if (!p) return false;
  if (p.startsWith("?")) return true;
  const tail = p.length >= 9 ? p.slice(-9) : p;
  if (tail.length === 9 && tail[1] !== "w") return true;
  return false;
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
  externalDragHovered,
}: PanelProps) {
  const appState = useAppState();
  const dispatch = useAppDispatch();
  const [gitOpen, setGitOpen] = useState(false);
  const [gitActionsOpen, setGitActionsOpen] = useState(false);
  const [gitActivity, setGitActivity] = useState<GitActivity | null>(null);
  const gitActivityStartRef = useRef(panel.git);
  const [gridMetrics, setGridMetrics] = useState<{ cols: number; cardWidth: number; cardHeight: number; pageRows: number }>({ cols: 1, cardWidth: GRID_CARD_MIN_WIDTH, cardHeight: 168, pageRows: 1 });
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const restoreInlineStageFocusRef = useRef(false);
  const bookmarkBtnRef = useRef<HTMLButtonElement>(null);
  const rightDrag = useRef({ active: false, everMoved: false, startRow: -1, mode: "select" as "select" | "deselect", preSelection: [] as number[] });
  const termFocusFnRef = useRef<(() => void) | null>(null);
  const prevTerminalSessionRef = useRef(panel.terminalSessionId);
  const contextMenuOpenRef = useRef(false);
  const contextMenuHoldStartRef = useRef(0);
  const contextMenuPendingRef = useRef<null | {
    entry: VFSEntry;
    absIndex: number;
    rect: DOMRect;
    targetScope: "entry" | "selection";
    targetCount: number;
  }>(null);
  const suppressNextContextMenuRef = useRef(false);
  const calcSet = new Set(panel.calculatingSize ?? []);
  const [tooltipCard, setTooltipCard] = useState<{
    absIndex: number;
    top: number;
    left: number;
    width: number;
  } | null>(null);
  const [tooltipNameClipped, setTooltipNameClipped] = useState(false);
  const tooltipNameRef = useRef<HTMLDivElement | null>(null);

  const restorePanelFocus = useCallback(() => {
    // The dialog owns focus while it is open. Return it to the panel on the
    // next frame, after React has removed the dialog from the DOM.
    requestAnimationFrame(() => focusDivRef.current?.focus());
  }, []);
  const closeGitModal = useCallback(() => {
    setGitOpen(false);
    setGitActivity(null);
    restorePanelFocus();
  }, [restorePanelFocus]);
  const closeGitActions = useCallback(() => {
    setGitActionsOpen(false);
    setGitActivity(null);
    restorePanelFocus();
  }, [restorePanelFocus]);
  const beginGitActivity = useCallback((activity: GitActivity) => {
    gitActivityStartRef.current = panel.git;
    setGitActivity(activity);
  }, [panel.git]);
  const openGitRepository = useCallback(() => {
    beginGitActivity({ action: "GitRefresh", message: "Fetching and refreshing…" });
    client.sendIntent("GitRefresh", undefined, { panelId });
    setGitOpen(true);
  }, [beginGitActivity, client, panelId]);
  const openGitActions = useCallback(() => {
    beginGitActivity({ action: "GitRefresh", message: "Fetching and refreshing…" });
    client.sendIntent("GitRefresh", undefined, { panelId });
    setGitActionsOpen(true);
  }, [beginGitActivity, client, panelId]);

  useEffect(() => {
    if (!gitActivity || gitActivity.action === "complete" || !panel.git || panel.git === gitActivityStartRef.current) return;
    setGitActivity({
      action: "complete",
      message: `${gitActivity.message.replace(/…$/, "")} complete`,
    });
    gitActivityStartRef.current = panel.git;
  }, [gitActivity, panel.git]);

  const isWindowed = panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
  const totalCount = panel.totalCount ?? panel.entries.length;
  const windowStart = panel.windowStart ?? 0;
  const isGrid = panel.viewMode === "grid" && panel.supportsThumbnails;
  const gitHeaderState = panel.git ? (panel.git.clean ? "ready" : "warning") : null;
  const selectionSet = useMemo(() => new Set(panel.selection), [panel.selection]);

  const [filterInput, setFilterInput] = useState(panel.filter?.value ?? "");
  const filterInputRef = useRef<HTMLInputElement>(null);
  const filterDebounceRef = useRef<number>(0);
  const shouldFocusFilterRef = useRef(false);
  const sortSelectorOpen = appState.activeSortSelectorPanelId === panelId;
  const sortSelectorOpenRef = useRef(sortSelectorOpen);
  const [sortDraft, setSortDraft] = useState<PanelState["sort"]>(panel.sort);

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

  const requestInlineStageFocusRestore = useCallback(() => {
    restoreInlineStageFocusRef.current = true;
  }, []);

  useEffect(() => {
    if (!restoreInlineStageFocusRef.current) return;
    const frame = requestAnimationFrame(() => {
      const cursorRow = listRef.current?.querySelector<HTMLElement>(`[data-abs-index="${panel.cursor}"]`);
      const stageButton = cursorRow?.querySelector<HTMLButtonElement>('[data-testid="git-inline-stage"], [data-testid="git-inline-unstage"]');
      if (stageButton) {
        stageButton.focus();
        restoreInlineStageFocusRef.current = false;
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [panel.cursor, panel.entries, panel.status]);

  const closeSortSelector = useCallback(() => {
    if (appState.activeSortSelectorPanelId === panelId) {
      dispatch({ type: "CLOSE_SORT_SELECTOR" });
    }
  }, [appState.activeSortSelectorPanelId, dispatch, panelId]);

  const applySortSelector = useCallback(() => {
    client.sendIntent("SetSort", panelId, sortDraft);
    dispatch({ type: "CLOSE_SORT_SELECTOR" });
  }, [client, dispatch, panelId, sortDraft]);

  const handleSortSelectorKeyDown = useCallback((e: React.KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      closeSortSelector();
      return;
    }
    if (e.key === "Enter") {
      applySortSelector();
      return;
    }
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      setSortDraft((current) => ({ ...current, order: current.order === "asc" ? "desc" : "asc" }));
      return;
    }
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      setSortDraft((current) => {
        const idx = SORT_COLUMNS.findIndex((column) => column.id === current.by);
        const direction = e.key === "ArrowRight" ? 1 : -1;
        const nextIdx = (Math.max(0, idx) + direction + SORT_COLUMNS.length) % SORT_COLUMNS.length;
        return { ...current, by: SORT_COLUMNS[nextIdx].id };
      });
    }
  }, [applySortSelector, closeSortSelector]);

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

  // Track grid metrics (cols/cardWidth/cardHeight) as the panel resizes so
  // absolute card positions, the virtual sizer, and RequestWindow math all
  // agree on geometry.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const update = () => {
      const m = computeGridMetrics(list);
      setGridMetrics((prev) =>
        prev.cols === m.cols && prev.cardWidth === m.cardWidth && prev.cardHeight === m.cardHeight && prev.pageRows === m.pageRows
          ? prev
          : m,
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(list);
    return () => ro.disconnect();
  }, []);

  // `g` changes the rendering mode without necessarily resizing this element,
  // so ResizeObserver alone can leave a stale column count in place. Refresh
  // before the first grid paint to keep the row/column coordinates coherent.
  useLayoutEffect(() => {
    if (!isGrid) return;
    const list = listRef.current;
    if (!list) return;
    const metrics = computeGridMetrics(list);
    setGridMetrics((previous) =>
      previous.cols === metrics.cols
        && previous.cardWidth === metrics.cardWidth
        && previous.cardHeight === metrics.cardHeight
        && previous.pageRows === metrics.pageRows
        ? previous
        : metrics,
    );
  }, [isGrid, panelId]);

  // Cursor-dwell tooltip: in grid mode, after the listing cursor stays on a
  // card for 2 s and that card has any ellipsed text, render an expanded
  // clone of the card showing the full content. Hidden the moment the
  // cursor moves, the panel loses focus, or the panel leaves grid mode.
  // Expanded grid card: clicking a clipped card renders a wider clone with
  // full metadata. Cursor movement, focus loss, and leaving grid mode hide it.
  const isGridView = panel.viewMode === "grid" && panel.supportsThumbnails;
  useEffect(() => {
    if (!isGridView || !isFocused) {
      setTooltipCard(null);
    }
  }, [isGridView, isFocused]);

  useEffect(() => {
    setTooltipCard((current) => current && current.absIndex !== panel.cursor ? null : current);
  }, [panel.cursor]);

  useEffect(() => {
    setTooltipCard(null);
  }, [panel.path, panel.sourceId]);

  // After the tooltip mounts/updates, check whether the name is *still*
  // clipped (its 2-line clamp is overflowing). If so, expose an [i] button
  // that opens the file-info modal — same behaviour as pressing `i`.
  useLayoutEffect(() => {
    if (!tooltipCard) {
      if (tooltipNameClipped) setTooltipNameClipped(false);
      return;
    }
    const el = tooltipNameRef.current;
    if (!el) return;
    const clipped = el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1;
    if (clipped !== tooltipNameClipped) setTooltipNameClipped(clipped);
  }, [tooltipCard, tooltipNameClipped]);

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
    if (sortSelectorOpen) {
      setSortDraft(panel.sort);
    }
  }, [sortSelectorOpen, panel.sort]);

  useEffect(() => {
    if (sortSelectorOpen && (!isFocused || modalActive || panel.filterModeActive)) {
      dispatch({ type: "CLOSE_SORT_SELECTOR" });
    }
  }, [dispatch, isFocused, modalActive, panel.filterModeActive, sortSelectorOpen]);

  useEffect(() => {
    sortSelectorOpenRef.current = sortSelectorOpen;
  }, [sortSelectorOpen]);

  useEffect(() => {
    return () => {
      if (sortSelectorOpenRef.current) {
        dispatch({ type: "CLOSE_SORT_SELECTOR" });
      }
    };
  }, [dispatch]);

  // Restore focus to the listing when the embedded terminal closes.
  useEffect(() => {
    const prev = prevTerminalSessionRef.current;
    prevTerminalSessionRef.current = panel.terminalSessionId;
    if (prev !== undefined && panel.terminalSessionId === undefined) {
      if (isFocused) focusDivRef.current?.focus();
      if (appState.maximizedTerminalPanelId === panelId) {
        dispatch({ type: "SET_MAXIMIZED_TERMINAL", panelId: null });
      }
    }
  }, [panel.terminalSessionId, isFocused, appState.maximizedTerminalPanelId, panelId, dispatch]);

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

  // Window request on scroll — coalesced to one per animation frame so a fast
  // scrollbar drag doesn't saturate the WS channel with hundreds of intents.
  // The window-slice patches now ride on their own monotonic windowVersion
  // (decoupled from the structural version guard), so missing one mid-drag
  // is harmless — the next frame resends with the latest scrollTop.
  const windowRafRef = useRef(0);
  const isGridMode = panel.viewMode === "grid" && panel.supportsThumbnails;
  const handleScroll = useCallback(() => {
    if (!isWindowed && !isGridMode) return;
    if (windowRafRef.current) return;
    windowRafRef.current = requestAnimationFrame(() => {
      windowRafRef.current = 0;
      const list = listRef.current;
      if (!list) return;
      const scrollTop = list.scrollTop;
      const viewportHeight = list.clientHeight;
      if (isGridMode) {
        const { cols, cardHeight } = computeGridMetrics(list);
        const rowStride = cardHeight + GRID_GAP;
        const firstVisibleRow = Math.floor(scrollTop / rowStride);
        const visibleRows = Math.ceil(viewportHeight / rowStride);
        const bufferRows = visibleRows; // 1 screen on each side
        const startRow = Math.max(0, firstVisibleRow - bufferRows);
        const start = startRow * cols;
        const count = (visibleRows + 2 * bufferRows) * cols;
        client.sendIntent("RequestWindow", panelId, { start, count });
        return;
      }
      const firstVisible = Math.floor(scrollTop / ROW_HEIGHT);
      const visibleCount = Math.ceil(viewportHeight / ROW_HEIGHT);
      const buffer = visibleCount; // 1 screen buffer on each side
      const start = Math.max(0, firstVisible - buffer);
      const count = visibleCount + 2 * buffer;
      client.sendIntent("RequestWindow", panelId, { start, count });
    });
  }, [isWindowed, isGridMode, panelId, client]);

  useEffect(() => () => {
    if (windowRafRef.current) {
      cancelAnimationFrame(windowRafRef.current);
      // Reset the ref too — otherwise (notably under React StrictMode in dev,
      // which runs every effect mount/cleanup/mount) the cancelled rAF leaves
      // the ref holding a phantom non-zero handle. The next mount's
      // handleScroll then sees a "pending" rAF that will never fire and
      // silently coalesces every scroll forever.
      windowRafRef.current = 0;
    }
  }, []);

  // Scroll cursor into view and ensure the window covers the scroll position.
  // Also fires when isWindowed transitions (e.g. sort/filter completes) so the
  // correct window is always loaded for whatever scrollTop the list has.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;

    const isGridView = panel.viewMode === "grid" && panel.supportsThumbnails;

    if (isGridView) {
      // Cards are absolute-positioned at row*(cardH+gap), so we can scroll
      // to any cursor — even one that hasn't been delivered yet — and let
      // handleScroll() request the window for the new position.
      const { cols, cardHeight } = gridMetrics;
      const cardRow = Math.floor(panel.cursor / Math.max(1, cols));
      const cardTop = cardRow * (cardHeight + GRID_GAP);
      const cardBottom = cardTop + cardHeight;
      if (cardTop < list.scrollTop) {
        list.scrollTop = cardTop;
      } else if (cardBottom > list.scrollTop + list.clientHeight) {
        list.scrollTop = cardBottom - list.clientHeight;
      }
      handleScroll();
      return;
    }

    if (isWindowed) {
      // In windowed mode, use absolute cursor position to scroll
      const cursorTop = panel.cursor * ROW_HEIGHT;
      const cursorBottom = cursorTop + ROW_HEIGHT;
      if (cursorTop < list.scrollTop) {
        list.scrollTop = cursorTop;
      } else if (cursorBottom > list.scrollTop + list.clientHeight) {
        list.scrollTop = cursorBottom - list.clientHeight;
      }
      // After any cursor/isWindowed change, reload the window for the
      // (possibly just-scrolled-to) position.
      handleScroll();
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
  }, [panel.cursor, panel.viewMode, panel.supportsThumbnails, gridMetrics, isWindowed, handleScroll]);


  const openContextMenuForEntry = useCallback((
    entry: VFSEntry,
    absIndex: number,
    rect: DOMRect,
    explicitTarget?: { targetScope: "entry" | "selection"; targetCount: number },
  ) => {
    const dirPath = panel.path.replace(/[/\\]+$/, "");
    const fullPath = entry.sourcePath ?? (entry.name === ".." ? dirPath : `${dirPath}/${entry.name}`);
    if (entry.name === "..") return;
    const targetsSelection = explicitTarget
      ? explicitTarget.targetScope === "selection"
      : panel.selection.includes(absIndex);
    const targetCount = explicitTarget?.targetCount ?? (targetsSelection ? panel.selection.length : 1);

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
        isShadow: !!panel.shadowSource,
        targetScope: targetsSelection ? "selection" : "entry",
        targetCount,
      },
    });
  }, [client, dispatch, isFocused, panel.path, panel.selection, panel.shadowSource, panelId]);

  const openContextMenuAtCursor = useCallback(() => {
    const entry = panel.entries[panel.cursor - (panel.windowStart ?? 0)];
    if (!entry || entry.name === "..") return;

    const row = listRef.current?.querySelector<HTMLElement>(`[data-abs-index="${panel.cursor}"]`);
    const rect = row?.getBoundingClientRect() ?? focusDivRef.current?.getBoundingClientRect();
    if (!rect) return;
    const dirPath = panel.path.replace(/[/\\]+$/, "");
    const fullPath = entry.sourcePath ?? `${dirPath}/${entry.name}`;
    const targetsSelection = panel.selection.length > 0;
    dispatch({
      type: "OPEN_CONTEXT_MENU",
      menu: {
        panelId,
        absIndex: panel.cursor,
        name: entry.name,
        kind: entry.kind,
        size: entry.size,
        dirPath,
        fullPath,
        anchorLeft: rect.left,
        anchorRight: rect.right,
        anchorTop: rect.top,
        anchorBottom: rect.bottom,
        isShadow: !!panel.shadowSource,
        targetScope: targetsSelection ? "selection" : "entry",
        targetCount: targetsSelection ? panel.selection.length : 1,
      },
    });
  }, [dispatch, panel, panelId]);

  // End right-drag on mouse up anywhere; if the right button was held long
  // enough without becoming a drag, open the contextual menu on release.
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 2) {
        const heldLongEnough = contextMenuHoldStartRef.current > 0 && Date.now() - contextMenuHoldStartRef.current >= 400;
        const pending = contextMenuPendingRef.current;
        if (pending && heldLongEnough && !rightDrag.current.everMoved) {
          suppressNextContextMenuRef.current = true;
          openContextMenuForEntry(pending.entry, pending.absIndex, pending.rect, {
            targetScope: pending.targetScope,
            targetCount: pending.targetCount,
          });
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
    // Keys typed inside the embedded terminal must not trigger panel shortcuts —
    // otherwise typing a literal `t` into the shell would close the terminal.
    if (e.target instanceof Element && e.target.closest(".panel-terminal")) {
      return;
    }
    if (sortSelectorOpen) {
      handleSortSelectorKeyDown(e);
      return;
    }
    const unmodified = !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey;
    if (unmodified && e.key === "ArrowRight" && panel.viewMode === "list") {
      const cursorRow = listRef.current?.querySelector<HTMLElement>(`[data-abs-index="${panel.cursor}"]`);
      const diffButton = cursorRow?.querySelector<HTMLButtonElement>('[data-testid="git-inline-view-diff"]');
      if (diffButton) {
        e.preventDefault();
        e.stopPropagation();
        diffButton.focus();
        return;
      }
    }
    const gitActionsCommand = findMatchingCommand(appState.commands, ["panel.git.actions"], e.nativeEvent, panel.viewMode);
    if (gitActionsCommand) {
      e.preventDefault();
      e.stopPropagation();
      if (panel.git) openGitActions();
      else dispatch({ type: "SHOW_TRANSIENT_HINT", message: "Git actions are available inside a local Git repository" });
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
    // When the terminal is open and the listing has focus (after Esc), pressing t
    // should re-focus the terminal rather than closing it.
    if (panel.terminalSessionId !== undefined && termFocusFnRef.current) {
      const toggleCmd = findMatchingCommand(appState.commands, ["panel.toggle_embedded_terminal"], e.nativeEvent, panel.viewMode);
      if (toggleCmd) {
        e.preventDefault();
        e.stopPropagation();
        termFocusFnRef.current();
        return;
      }
    }
    const command = findMatchingCommand(appState.commands, PANEL_SHORTCUT_COMMAND_IDS, e.nativeEvent, panel.viewMode);
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
        onOpenContextMenu: () => {
          openContextMenuAtCursor();
        },
        getPanelPageSize: () => {
          const list = listRef.current;
          return list ? Math.max(1, Math.floor(list.clientHeight / ROW_HEIGHT)) : 1;
        },
        getGridCols: () => computeGridMetrics(listRef.current).cols,
        getGridPageRows: () => computeGridMetrics(listRef.current).pageRows,
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

    const matchingPanelCommand = findMatchingCommand(appState.commands, PANEL_SHORTCUT_COMMAND_IDS, e.nativeEvent, panel.viewMode);
    if (matchingPanelCommand) {
      focusPanel();
      return;
    }

    const matchingGlobalCommand = findMatchingCommand(appState.commands, GLOBAL_SHORTCUT_COMMAND_IDS, e.nativeEvent, panel.viewMode);
    if (matchingGlobalCommand) {
      return;
    }

    e.stopPropagation();
  };

  const isGlobalSelectionsRoot = isGlobalSelectionSourceId(panel.sourceId) && panel.path === "";
  const itemCount = isWindowed ? totalCount : panel.entries.length;
  // In non-windowed mode recompute total from live entries so calculated dir sizes are included.
  const totalSize = isGlobalSelectionsRoot
    ? undefined
    : !isWindowed
      ? panel.entries.reduce((sum, e) => e.size !== undefined ? sum + e.size : sum, 0)
      : panel.totalSize;
  const selCount = panel.selection.length;
  // In windowed mode entries outside the window aren't available on the FE, so skip sel size.
  const selSize = !isGlobalSelectionsRoot && !isWindowed && selCount > 0
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

  const entryPathFor = (entry: VFSEntry): string | null => {
    if (entry.sourcePath) return entry.sourcePath;
    if (entry.name === "..") return null;
    if (entry.kind !== "file") return null;
    const dir = panel.path.replace(/[/\\]+$/, "");
    return `${dir}/${entry.name}`;
  };

  const thumbUrlFor = (entry: VFSEntry): string | null => {
    const entryPath = entryPathFor(entry);
    if (!entryPath || appState.fileServerPort <= 0) return null;
    if (source?.kind === "remote") {
      return `http://127.0.0.1:${appState.fileServerPort}/__remote_thumb__/${encodeURIComponent(panel.sourceId)}/${GRID_THUMB_SIZE}/${encodeURIComponent(entry.mtime ?? "0")}/${encodeURIComponent(String(entry.size ?? 0))}/${encodeURIComponent(entryPath)}`;
    }
    return `http://127.0.0.1:${appState.fileServerPort}/__thumb__/${GRID_THUMB_SIZE}/${encodeURIComponent(entryPath)}`;
  };

  const openEntry = (entry: VFSEntry, absIndex: number) => {
    setTooltipCard(null);
    dispatch({ type: "CLOSE_CONTEXT_MENU" });
    client.sendIntent("SetCursor", "focused-panel", { index: absIndex });
    if (entry.kind === "directory") {
      client.sendIntent("OpenEntry", "focused-panel");
      return;
    }
    if (source?.kind === "remote" || panel.sourceId === "journal") {
      client.sendIntent("ViewFile", undefined, {
        panelId,
        path: panel.path,
        sourceId: panel.sourceId,
        filename: entry.name,
      });
      return;
    }
    client.sendIntent("OpenFile", "focused-panel");
  };

  const showExpandedCardForClick = (absIndex: number) => {
    if (!isGrid) return;
    const list = listRef.current;
    if (!list) return;
    const card = list.querySelector<HTMLElement>(`[data-abs-index="${absIndex}"]`);
    if (!card) return;

    const overflowing = Array.from(
      card.querySelectorAll<HTMLElement>(".panel__card-name, .panel__card-size, .panel__card-date"),
    ).some((el) => el.scrollWidth > el.clientWidth + 1);
    if (!overflowing) {
      setTooltipCard(null);
      return;
    }

    const { cols, cardWidth, cardHeight } = gridMetrics;
    const safeCols = Math.max(1, cols);
    const row = Math.floor(absIndex / safeCols);
    const col = absIndex % safeCols;
    const innerWidth = Math.max(GRID_CARD_MIN_WIDTH, list.clientWidth - GRID_PADDING);
    const tooltipWidth = Math.min(cardWidth * 2, innerWidth);
    const cardLeft = (GRID_PADDING / 2) + col * (cardWidth + GRID_GAP);
    const cardCenter = cardLeft + cardWidth / 2;
    const desiredLeft = cardCenter - tooltipWidth / 2;
    const maxLeft = (GRID_PADDING / 2) + innerWidth - tooltipWidth;
    const left = Math.max(GRID_PADDING / 2, Math.min(desiredLeft, maxLeft));
    const top = row * (cardHeight + GRID_GAP);
    setTooltipCard({ absIndex, top, left, width: tooltipWidth });
  };

  function rowEventHandlers(entry: VFSEntry, absIndex: number) {
    return {
      onMouseDown: (e: React.MouseEvent) => {
        if (e.button === 2) {
          const rect = DOMRect.fromRect({
            x: e.clientX,
            y: e.clientY,
            width: 0,
            height: 0,
          });
          if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
          const mode = selectionSet.has(absIndex) ? "deselect" : "select";
          const targetsSelection = selectionSet.has(absIndex);
          rightDrag.current = { active: true, everMoved: false, startRow: absIndex, mode, preSelection: [...panel.selection] };
          contextMenuOpenRef.current = false;
          contextMenuHoldStartRef.current = Date.now();
          contextMenuPendingRef.current = entry.name === ".." ? null : {
            entry,
            absIndex,
            rect,
            targetScope: targetsSelection ? "selection" : "entry",
            targetCount: targetsSelection ? panel.selection.length : 1,
          };
          focusDivRef.current?.focus();
          return;
        }
        e.preventDefault();
        dispatch({ type: "CLOSE_CONTEXT_MENU" });
        if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
        if (e.shiftKey) {
          // Shift+click: union linear range from current cursor to this item.
          // Same absolute-index range in list and grid (not a 2D rectangle).
          client.sendIntent("ExtendSelectionTo", panelId, { index: absIndex });
          focusDivRef.current?.focus();
          return;
        }
        client.sendIntent("SetCursor", panelId, { index: absIndex });
        onRowPointerDown({ absIndex, clientX: e.clientX, clientY: e.clientY });
        focusDivRef.current?.focus();
      },
      onMouseEnter: () => {
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
      },
      onContextMenu: (e: React.MouseEvent) => {
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
        if (contextMenuHoldStartRef.current > 0 && Date.now() - contextMenuHoldStartRef.current >= 400) {
          return;
        }
        if (!rightDrag.current.everMoved) {
          client.sendIntent("ToggleSelectionAtIndex", panelId, { index: absIndex });
        }
      },
      onDoubleClick: () => {
        openEntry(entry, absIndex);
      },
      onClick: () => {
        showExpandedCardForClick(absIndex);
      },
    };
  }

  function renderCard(
    entry: VFSEntry,
    absIndex: number,
    pos?: { top: number; left: number; width: number; height: number },
    options?: { tooltip?: boolean; thumbWidth?: number },
  ) {
    const isTooltip = options?.tooltip === true;
    const handlers = isTooltip ? null : rowEventHandlers(entry, absIndex);
    const colorClass = colorClassForEntry(entry);
    const baseStyle: React.CSSProperties | undefined = pos
      ? { position: "absolute", top: pos.top, left: pos.left, width: pos.width, height: pos.height }
      : undefined;
    // Pin the thumb to the original card width inside the tooltip so the
    // wider tooltip card doesn't grow the thumbnail.
    const style = isTooltip && options?.thumbWidth !== undefined
      ? { ...(baseStyle ?? {}), ["--panel-thumb-w" as any]: `${options.thumbWidth}px` }
      : baseStyle;
    return (
      <div
        // The tooltip is a parallel render of the same card; give it a
        // distinct key so React doesn't try to reconcile it against the
        // underlying card sharing the same absIndex.
        key={isTooltip ? `tooltip-${absIndex}` : absIndex}
        role={isTooltip ? undefined : "row"}
        data-testid={isTooltip ? undefined : "panel-entry"}
        data-abs-index={isTooltip ? undefined : absIndex}
        data-entry-name={isTooltip ? undefined : entry.name}
        data-entry-kind={isTooltip ? undefined : entry.kind}
        className={[
          "panel__card",
          isTooltip ? "panel__card--tooltip" : "",
          !isTooltip && absIndex === panel.cursor ? "panel__card--cursor" : "",
          !isTooltip && selectionSet.has(absIndex) ? "panel__card--selected" : "",
          !isTooltip && dragActive && dragDropAllowed && dragHoverAbsIndex === absIndex ? "panel__card--drop-target" : "",
        ].filter(Boolean).join(" ")}
        style={style}
        {...(handlers ?? {})}
      >
        <EntryThumb entry={entry} thumbUrl={thumbUrlFor(entry)} />
        {isTooltip ? (
          <div className="panel__card-name-row">
            <div
              ref={tooltipNameRef}
              className={`panel__card-name${colorClass ? ` ${colorClass}` : ""}`}
              title={entry.name}
            >
              {entry.name}
            </div>
            {tooltipNameClipped && (
              <button
                type="button"
                className="panel__card-info-btn"
                title="File info (i)"
                onMouseDown={(e) => {
                  // Don't let the listing's cursor / focus logic intercept.
                  e.stopPropagation();
                  e.preventDefault();
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  const dirPath = panel.path.replace(/[/\\]+$/, "");
                  const fullPath = entry.sourcePath
                    ? entry.sourcePath
                    : entry.name === ".."
                      ? dirPath
                      : `${dirPath}/${entry.name}`;
                  dispatch({
                    type: "OPEN_INFO",
                    info: {
                      name: entry.name,
                      kind: entry.kind,
                      fullPath,
                      dirPath,
                      size: entry.size,
                      mtime: entry.mtime,
                      btime: entry.btime,
                      permissions: entry.permissions,
                      iconName: entry.iconName,
                    },
                  });
                }}
              >
                i
              </button>
            )}
          </div>
        ) : (
          <div className={`panel__card-name${colorClass ? ` ${colorClass}` : ""}`} title={entry.name}>{entry.name}</div>
        )}
        <div className="panel__card-meta">
          <span className="panel__card-size">{formatSize(entry, calcSet, isGlobalSelectionsRoot)}</span>
          <span className="panel__card-date">{formatMtime(entry.mtime)}</span>
        </div>
      </div>
    );
  }

  function renderRow(entry: VFSEntry, absIndex: number) {
    const colorClass = colorClassForEntry(entry);
    const gitDetails = entry.gitDetails;
    const gitSummary = gitDetails
      ? [
          `${gitDetails.changedCount} changed file${gitDetails.changedCount === 1 ? "" : "s"}`,
          ...gitDetails.samplePaths,
          ...(gitDetails.changedCount > gitDetails.samplePaths.length ? ["…"] : []),
        ].join("\n")
      : undefined;
    const gitInline = gitDetails
      ? entry.kind === "directory" && gitDetails.samplePaths[0]
        ? `${gitDetails.changedCount} · ${gitDetails.samplePaths[0]}`
        : entry.gitStatus === "untracked"
          ? `${gitDetails.changedCount} untracked file${gitDetails.changedCount === 1 ? "" : "s"}`
          : `${gitDetails.changedCount} change${gitDetails.changedCount === 1 ? "" : "s"}`
      : undefined;
    const inlineGitPath = entry.kind === "file" && gitDetails?.changedCount === 1
      ? gitDetails.samplePaths[0]
      : undefined;
    const inlineGitChange = inlineGitPath
      ? panel.git?.changes.find(change => change.path === inlineGitPath)
      : undefined;
    return (
      <div
        key={absIndex}
        role="row"
        data-testid="panel-entry"
        data-abs-index={absIndex}
        data-entry-name={entry.name}
        data-entry-kind={entry.kind}
        className={[
          "panel__row",
          absIndex % 2 === 1 ? "panel__row--alt" : "",
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
            const targetsSelection = selectionSet.has(absIndex);
            rightDrag.current = { active: true, everMoved: false, startRow: absIndex, mode, preSelection: [...panel.selection] };
            contextMenuOpenRef.current = false;
            contextMenuHoldStartRef.current = Date.now();
            contextMenuPendingRef.current = entry.name === ".." ? null : {
              entry,
              absIndex,
              rect,
              targetScope: targetsSelection ? "selection" : "entry",
              targetCount: targetsSelection ? panel.selection.length : 1,
            };
            focusDivRef.current?.focus();
            return;
          }
          e.preventDefault();
          dispatch({ type: "CLOSE_CONTEXT_MENU" });
          if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
          if (e.shiftKey) {
            // Shift+click: union linear range from current cursor to this item.
            // Same absolute-index range in list and grid (not a 2D rectangle).
            client.sendIntent("ExtendSelectionTo", panelId, { index: absIndex });
            focusDivRef.current?.focus();
            return;
          }
          client.sendIntent("SetCursor", panelId, { index: absIndex });
          onRowPointerDown({ absIndex, clientX: e.clientX, clientY: e.clientY });
          focusDivRef.current?.focus();
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
          if (contextMenuHoldStartRef.current > 0 && Date.now() - contextMenuHoldStartRef.current >= 400) {
            return;
          }
          if (!rightDrag.current.everMoved) {
            client.sendIntent("ToggleSelectionAtIndex", panelId, { index: absIndex });
          }
        }}
        onDoubleClick={() => {
          openEntry(entry, absIndex);
        }}
      >
        <EntryIcon entry={entry} />
        {(() => {
          const { base, ext } = splitNameExt(entry);
          const restricted = isRestricted(entry);
          return (
            <span className={`panel__name${colorClass ? ` ${colorClass}` : ""}`}>
              <span className="panel__name-base">{base}</span>
              {ext && <span className="panel__name-ext">{ext}</span>}
              {restricted && (
                <button
                  type="button"
                  className="panel__name-warn"
                  title={entry.permissions ?? ""}
                  onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
                  onClick={(e) => {
                    e.stopPropagation();
                    const dirPath = panel.path.replace(/[/\\]+$/, "");
                    const fullPath = entry.sourcePath
                      ? entry.sourcePath
                      : entry.name === ".."
                        ? dirPath
                        : `${dirPath}/${entry.name}`;
                    dispatch({
                      type: "OPEN_INFO",
                      info: {
                        name: entry.name,
                        kind: entry.kind,
                        fullPath,
                        dirPath,
                        size: entry.size,
                        mtime: entry.mtime,
                        btime: entry.btime,
                        permissions: entry.permissions,
                        iconName: entry.iconName,
                      },
                    });
                  }}
                >
                  <IconAlertTriangle size={15} />
                </button>
              )}
              {entry.gitStatus && <span className="panel__git-status" data-tooltip-id={`git-summary-${panelId}`} data-tooltip-content={gitSummary ? `Git: ${entry.gitStatus}\n${gitSummary}` : `Git: ${entry.gitStatus}`}>{entry.gitStatus === "untracked" ? "?" : entry.gitStatus === "modified" ? "M" : entry.gitStatus === "deleted" ? "D" : entry.gitStatus === "conflicted" ? "!" : "●"}</span>}
              {absIndex === panel.cursor && gitInline && (inlineGitPath && inlineGitChange
                ? <GitBubbleActions panelId={panelId} path={inlineGitPath} label={gitInline} staged={inlineGitChange.indexStatus !== " " && inlineGitChange.indexStatus !== "?"} client={client} onExitKeyboardMode={focusPanel} onRestoreStageFocus={requestInlineStageFocusRestore} />
                : <span className="panel__git-bubble" data-tooltip-id={`git-summary-${panelId}`} data-tooltip-content={gitSummary}>{gitInline}</span>)}
            </span>
          );
        })()}
        <span className="panel__size">{formatSize(entry, calcSet, isGlobalSelectionsRoot)}</span>
        <span className="panel__mtime">{formatMtime(entry.mtime)}</span>
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
        externalDragHovered ? "panel--external-drag-hover" : "",
      ].filter(Boolean).join(" ")}
      data-testid={`panel-${panelId}`}
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
      {(() => {
        const slot = appState.slots[panel.slotId];
        if (!slot) return null;
        // Show the tab bar in every panel whenever any panel has >1 tab — keeps
        // breadcrumb / listing rows aligned across panels.
        const anyTabbed = Object.values(appState.slots).some(
          (s) => s.tabPanelIds.length > 1,
        );
        if (!anyTabbed) return null;
        return <PanelTabBar slot={slot} panels={appState.panels} client={client} />;
      })()}

      {panel.tabKind === "terminal" && panel.terminalSessionId !== undefined ? (
        <PanelTerminal
          client={client}
          panelId={panelId}
          sessionId={panel.terminalSessionId}
          fontSize={appState.fontSize}
          colorScheme={appState.settings.general.colorScheme}
          solo
          maximized={appState.maximizedTerminalPanelId === panelId}
          onToggleMaximize={() => dispatch({
            type: "SET_MAXIMIZED_TERMINAL",
            panelId: appState.maximizedTerminalPanelId === panelId ? null : panelId,
          })}
          onFocusRef={(fn) => { termFocusFnRef.current = fn; }}
          onClose={() => client.sendIntent("panel.tab.close", undefined, { panelId })}
          onBlurToListing={() => focusDivRef.current?.focus()}
        />
      ) : (
      <>
      <div className={`panel__header${gitHeaderState ? ` panel__header--git-${gitHeaderState}` : ""}`}>
        <PathBreadcrumbs panelId={panelId} path={panel.path} client={client} shadowSource={panel.shadowSource} />
        <button
          className="panel__home-btn"
          tabIndex={-1}
          title="New tab (Ctrl+T)"
          onClick={() => client.sendIntent("panel.tab.new", undefined, { slotId: panel.slotId, source: "clone" })}
        >+</button>
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
        <button
          ref={bookmarkBtnRef}
          className={`panel__bookmark-btn${appState.bookmarks.some((b) => b.path === panel.path) ? " panel__bookmark-btn--active" : ""}`}
          tabIndex={-1}
          title="Bookmarks (Ctrl+D)"
          onClick={() => {
            if (appState.bookmarksOpen?.panelId === panelId) dispatch({ type: "CLOSE_BOOKMARKS" });
            else dispatch({ type: "OPEN_BOOKMARKS", panelId });
          }}
        >★</button>
        {appState.bookmarksOpen?.panelId === panelId && (
          <BookmarksPopover
            anchorEl={bookmarkBtnRef.current}
            panelId={panelId}
            currentPath={panel.path}
            bookmarks={appState.bookmarks}
            client={client}
          />
        )}
        {panel.git && <button className={`panel__git-btn panel__git-btn--${gitHeaderState}`} tabIndex={-1} data-tooltip-id={`git-chip-${panelId}`} data-tooltip-content={panel.git.clean ? `Git ${panel.git.branch}: no staged changes` : `Git ${panel.git.branch}: staged changes present — commit or unstage before switching or pulling`} onClick={openGitRepository}><IconGitBranch size={16} /><span className="panel__git-btn-label">{panel.git.branch}</span>{!panel.git.clean && <IconAlertTriangle className="panel__git-btn-warning" size={13} />}</button>}
      </div>

      {gitOpen && panel.git && <GitModal panelId={panelId} git={panel.git} client={client} activity={gitActivity} onActivityChange={beginGitActivity} onClose={closeGitModal} />}
      {gitActionsOpen && panel.git && <GitActionsModal panelId={panelId} git={panel.git} client={client} activity={gitActivity} onActivityChange={beginGitActivity} onClose={closeGitActions} onOpenRepository={() => { setGitActionsOpen(false); setGitOpen(true); }} />}

      <VolumeHeader sources={sources} panel={panel} client={client} onNavigateToSource={(sourceId) => { shouldFocusFilterRef.current = false; client.sendIntent("NavigateToSource", panelId, { sourceId }); }} onMountSource={(sourceId) => { shouldFocusFilterRef.current = false; client.sendIntent("MountSource", panelId, { sourceId }); }} />

      <SortHeader sort={panel.sort} panelId={panelId} client={client} />

      <div className={`panel__list${isGrid ? " panel__list--grid" : ""}`} ref={listRef} onScroll={isGrid || isWindowed ? handleScroll : undefined} onMouseDown={(e) => { if (e.target === e.currentTarget) focusDivRef.current?.focus(); }} style={modalActive ? { overflow: "hidden" } : undefined}>
        {isGrid ? (
          (() => {
            const { cols, cardWidth, cardHeight } = gridMetrics;
            const totalRows = Math.ceil(totalCount / Math.max(1, cols));
            const sizerHeight = totalRows * (cardHeight + GRID_GAP);
            return (
              <div className="panel__virtual-sizer" style={{ height: sizerHeight, position: "relative" }}>
                {panel.entries.map((entry, i) => {
                  const absIndex = windowStart + i;
                  const row = Math.floor(absIndex / Math.max(1, cols));
                  const col = absIndex % Math.max(1, cols);
                  const top = row * (cardHeight + GRID_GAP);
                  const left = (GRID_PADDING / 2) + col * (cardWidth + GRID_GAP);
                  return renderCard(entry, absIndex, { top, left, width: cardWidth, height: cardHeight });
                })}
                {tooltipCard && (() => {
                  const sliceIdx = tooltipCard.absIndex - windowStart;
                  const entry = sliceIdx >= 0 && sliceIdx < panel.entries.length ? panel.entries[sliceIdx] : null;
                  if (!entry) return null;
                  return renderCard(entry, tooltipCard.absIndex, {
                    top: tooltipCard.top,
                    left: tooltipCard.left,
                    width: tooltipCard.width,
                    height: cardHeight,
                  }, { tooltip: true, thumbWidth: cardWidth });
                })()}
                {panel.status === "loading" && panel.entries.length === 0 && (
                  <div className="panel__loading">Loading…</div>
                )}
              </div>
            );
          })()
        ) : isWindowed ? (
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

      {sortSelectorOpen && (
        <SortSelector draft={sortDraft} current={panel.sort} />
      )}

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
              <IconX size={12} />
            </button>
          )}
          <span className="panel__footer-status">{statusText}</span>
        </div>
        <div className="panel__footer-right">
          {panel.filter && <span className="panel__filter-active">/{panel.filter.value}/</span>}
          {!panel.showHidden && (
            <button
              type="button"
              className="panel__hidden-indicator"
              data-tooltip-id="hidden-files"
              data-tooltip-content="Hidden files are hidden (press . to show)"
              onClick={() => client.sendIntent("ToggleHidden", panelId, {})}
            >
              H
            </button>
          )}
          {diskText && (
            <span
              className="panel__disk-text"
              onClick={() => client.sendIntent("RequestStateSnapshot", undefined, {})}
            >
              {diskText}
            </span>
          )}
        </div>
      </div>
      <Tooltip id="hidden-files" delayShow={0} />
      <Tooltip id={`git-summary-${panelId}`} className="panel__git-tooltip" delayShow={120} place="bottom" border="1px solid var(--border-focus)" arrowColor="var(--bg-elev)" />
      <Tooltip id={`git-chip-${panelId}`} className="panel__git-tooltip" delayShow={120} place="bottom" border="1px solid var(--border-focus)" arrowColor="var(--bg-elev)" />
      <Tooltip id={`git-inline-actions-${panelId}`} className="panel__git-tooltip panel__git-icon-tooltip" delayShow={180} place="bottom" border="1px solid var(--border-focus)" arrowColor="var(--bg-elev)" />
      {panel.terminalSessionId !== undefined && (
        <PanelTerminal
          client={client}
          panelId={panelId}
          sessionId={panel.terminalSessionId}
          fontSize={appState.fontSize}
          colorScheme={appState.settings.general.colorScheme}
          maximized={appState.maximizedTerminalPanelId === panelId}
          onToggleMaximize={() => dispatch({
            type: "SET_MAXIMIZED_TERMINAL",
            panelId: appState.maximizedTerminalPanelId === panelId ? null : panelId,
          })}
          onFocusRef={(fn) => { termFocusFnRef.current = fn; }}
          onClose={() => client.sendIntent("ClosePanelTerminal", panelId)}
          onPromote={() => client.sendIntent("panel.terminal.promote", undefined, { panelId })}
          onBlurToListing={() => focusDivRef.current?.focus()}
        />
      )}
      </>
      )}
    </div>
  );
}
