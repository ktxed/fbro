import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type WheelEvent as ReactWheelEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { resolveWsUrl, WsClient } from "../ws/client";
import type {
  BEMessage,
  CompareDiffReadyMessage,
  CompareDiffResult,
  CompareDiffRow,
  StatePatchMessage,
  StateSnapshotMessage,
} from "../ws/types";
import { applyZoom, DEFAULT_FONT_SIZE } from "../utils/zoom";
import { DEFAULT_COLOR_SCHEME_ID, resolveColorScheme, type ColorSchemeId } from "../theme/colorSchemes";

type DiffViewMode = "split" | "unified";

const DIFF_ROW_HEIGHT = 13 * 1.45;
const DIFF_WHEEL_ROWS = 3;

interface DiffViewerAppProps {
  requestId: string;
  windowLabel: string;
  initialColorScheme?: string | null;
}

function shortPath(path: string) {
  const normalized = path.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  return parts.slice(-3).join("/") || path;
}

function lineText(value?: string) {
  return value === undefined || value === "" ? "\u00A0" : value;
}

function normalizedWheelDelta(delta: number, deltaMode: number, pageSize: number) {
  if (delta === 0) return 0;
  if (deltaMode === 1) return delta * DIFF_ROW_HEIGHT;
  if (deltaMode === 2) return delta * pageSize;
  if (Number.isInteger(delta) && Math.abs(delta) < DIFF_ROW_HEIGHT) {
    return Math.sign(delta) * DIFF_ROW_HEIGHT * DIFF_WHEEL_ROWS;
  }
  return delta;
}

function diffRowClassName(
  kind: CompareDiffRow["kind"],
  active: boolean,
  activeBlockStart: boolean,
  activeBlockEnd: boolean,
) {
  const classes = ["diff-table__row", `diff-table__row--${kind}`];
  if (active) classes.push("diff-table__row--active-change");
  if (activeBlockStart) classes.push("diff-table__row--active-change-start");
  if (activeBlockEnd) classes.push("diff-table__row--active-change-end");
  return classes.join(" ");
}

export function DiffViewerApp({ requestId, windowLabel, initialColorScheme }: DiffViewerAppProps) {
  const [result, setResult] = useState<CompareDiffResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<DiffViewMode>("split");
  const [loading, setLoading] = useState(true);
  const [activeChangeIndex, setActiveChangeIndex] = useState(0);
  const [colorSchemeId, setColorSchemeId] = useState<ColorSchemeId>(() => (
    resolveColorScheme(initialColorScheme ?? DEFAULT_COLOR_SCHEME_ID).id
  ));
  const clientRef = useRef<WsClient | null>(null);
  const currentFontSizeRef = useRef(DEFAULT_FONT_SIZE);
  const splitBodyRef = useRef<HTMLElement | null>(null);
  const unifiedBodyRef = useRef<HTMLElement | null>(null);
  const leftPaneRef = useRef<HTMLDivElement | null>(null);
  const rightPaneRef = useRef<HTMLDivElement | null>(null);
  const leftTableRef = useRef<HTMLTableElement | null>(null);
  const rightTableRef = useRef<HTMLTableElement | null>(null);
  const splitScrollSourceRef = useRef<HTMLDivElement | null>(null);
  const splitScrollReleaseRef = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    resolveWsUrl().then((wsUrl) => {
      if (cancelled) return;
      const client = new WsClient("viewer", wsUrl);
      clientRef.current = client;
      client.onConnected = () => {
        client.sendIntent("GetCompareDiff", undefined, { requestId });
      };
      client.onMessage = (msg: BEMessage) => {
        if (msg.type === "StateSnapshot") {
          const snapshot = msg as StateSnapshotMessage;
          const fontSize = snapshot.domains.fontSize;
          if (fontSize !== undefined) {
            currentFontSizeRef.current = fontSize;
            applyZoom(fontSize);
          }
          setColorSchemeId(resolveColorScheme(snapshot.domains.settings?.general.colorScheme).id);
        }
        if (msg.type === "StatePatch") {
          const patch = msg as StatePatchMessage;
          if (patch.domain === "settings" && patch.patch.fontSize !== undefined) {
            const fontSize = patch.patch.fontSize as number;
            currentFontSizeRef.current = fontSize;
            applyZoom(fontSize);
          }
          if (patch.domain === "settings") {
            const general = patch.patch.general as { colorScheme?: string } | undefined;
            if (general?.colorScheme !== undefined) {
              setColorSchemeId(resolveColorScheme(general.colorScheme).id);
            }
          }
        }
        if (msg.type === "CompareDiffReady") {
          const ready = msg as CompareDiffReadyMessage;
          if (ready.requestId !== requestId) return;
          setResult(ready.result);
          setError(null);
          setLoading(false);
        }
        if (msg.type === "CompareDiffFailed") {
          const failed = msg as { requestId: string; message: string };
          if (failed.requestId !== requestId) return;
          setError(failed.message);
          setLoading(false);
        }
      };
      client.connect();
    });
    return () => {
      cancelled = true;
      clientRef.current?.destroy();
      clientRef.current = null;
    };
  }, [requestId]);

  const closeWindow = () => {
    clientRef.current?.sendIntent("CloseCompareWindow", undefined, {
      requestId,
      label: windowLabel,
    });
    try {
      void getCurrentWindow().close();
    } catch {
      // Backend close handles Tauri contexts; browser tests can ignore it.
    }
  };

  const changedBlocks = useMemo(() => {
    if (!result) return [];
    const starts: number[] = [];
    result.rows.forEach((row, index) => {
      if (row.kind === "equal") return;
      if (index === 0 || result.rows[index - 1]?.kind === "equal") {
        starts.push(index);
      }
    });
    return starts;
  }, [result]);

  const changeBlockByRow = useMemo(() => {
    const blocks = new Map<number, number>();
    if (!result) return blocks;
    let blockIndex = -1;
    result.rows.forEach((row, index) => {
      if (row.kind === "equal") return;
      if (index === 0 || result.rows[index - 1]?.kind === "equal") {
        blockIndex += 1;
      }
      if (blockIndex >= 0) blocks.set(index, blockIndex);
    });
    return blocks;
  }, [result]);

  useEffect(() => {
    setActiveChangeIndex(0);
  }, [result]);

  const jumpChange = useCallback((delta: number) => {
    setActiveChangeIndex((current) => {
      if (changedBlocks.length === 0) return 0;
      return (current + delta + changedBlocks.length) % changedBlocks.length;
    });
  }, [changedBlocks.length]);

  const scrollDiffVertically = useCallback((destination: "start" | "end" | "page-up" | "page-down") => {
    const source = mode === "split" ? leftPaneRef.current : unifiedBodyRef.current;
    if (!source) return;
    let scrollTop: number;
    if (destination === "start") {
      scrollTop = 0;
    } else if (destination === "end") {
      scrollTop = source.scrollHeight;
    } else {
      const page = Math.max(DIFF_ROW_HEIGHT, source.clientHeight - DIFF_ROW_HEIGHT);
      scrollTop = source.scrollTop + (destination === "page-down" ? page : -page);
    }
    source.scrollTop = scrollTop;
    if (mode === "split" && rightPaneRef.current) {
      rightPaneRef.current.scrollTop = source.scrollTop;
    }
  }, [mode]);

  useEffect(() => {
    if (changedBlocks.length === 0) return;
    const activeRow = changedBlocks[activeChangeIndex] ?? changedBlocks[0];
    const el = document.querySelector<HTMLElement>(`[data-diff-row="${activeRow}"]`);
    el?.scrollIntoView({ block: "center", inline: "nearest" });
  }, [activeChangeIndex, changedBlocks, mode]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeWindow();
      }
      if (event.key === "1") setMode("split");
      if (event.key === "2") setMode("unified");
      if (event.key === "Home") {
        event.preventDefault();
        scrollDiffVertically("start");
        return;
      }
      if (event.key === "End") {
        event.preventDefault();
        scrollDiffVertically("end");
        return;
      }
      if (event.key === "PageUp") {
        event.preventDefault();
        scrollDiffVertically("page-up");
        return;
      }
      if (event.key === "PageDown") {
        event.preventDefault();
        scrollDiffVertically("page-down");
        return;
      }
      if (event.key === "n" || event.key === "j" || event.key === "F7" || event.key === "ArrowDown") {
        event.preventDefault();
        jumpChange(1);
      }
      if (event.key === "p" || event.key === "k" || event.key === "F6" || event.key === "ArrowUp") {
        event.preventDefault();
        jumpChange(-1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [jumpChange, scrollDiffVertically]);

  const colorScheme = resolveColorScheme(colorSchemeId);
  const rowCount = result?.rows.length ?? 0;
  const changedCount = useMemo(
    () => result?.rows.filter((row) => row.kind !== "equal").length ?? 0,
    [result],
  );

  const syncSplitScroll = useCallback((source: HTMLDivElement, target: HTMLDivElement | null) => {
    if (!target) return;
    if (splitScrollSourceRef.current && splitScrollSourceRef.current !== source) return;
    splitScrollSourceRef.current = source;
    if (target.scrollLeft !== source.scrollLeft) target.scrollLeft = source.scrollLeft;
    if (target.scrollTop !== source.scrollTop) target.scrollTop = source.scrollTop;
    if (splitScrollReleaseRef.current !== null) {
      cancelAnimationFrame(splitScrollReleaseRef.current);
    }
    splitScrollReleaseRef.current = requestAnimationFrame(() => {
      if (splitScrollSourceRef.current === source) splitScrollSourceRef.current = null;
      splitScrollReleaseRef.current = null;
    });
  }, []);

  const handleSplitWheel = useCallback((event: ReactWheelEvent<HTMLDivElement>, target: HTMLDivElement | null) => {
    if (!target || (event.deltaX === 0 && event.deltaY === 0)) return;
    event.preventDefault();
    const source = event.currentTarget;
    const horizontalInput = event.shiftKey && event.deltaX === 0 ? event.deltaY : event.deltaX;
    const verticalInput = event.shiftKey && event.deltaX === 0 ? 0 : event.deltaY;
    source.scrollBy({
      left: normalizedWheelDelta(horizontalInput, event.deltaMode, source.clientWidth),
      top: normalizedWheelDelta(verticalInput, event.deltaMode, source.clientHeight),
      behavior: "auto",
    });
    syncSplitScroll(source, target);
  }, [syncSplitScroll]);

  useEffect(() => () => {
    if (splitScrollReleaseRef.current !== null) {
      cancelAnimationFrame(splitScrollReleaseRef.current);
    }
  }, []);

  useLayoutEffect(() => {
    if (mode !== "split" || !result) return;
    const body = splitBodyRef.current;
    const leftPane = leftPaneRef.current;
    const rightPane = rightPaneRef.current;
    const leftTable = leftTableRef.current;
    const rightTable = rightTableRef.current;
    if (!body || !leftPane || !rightPane || !leftTable || !rightTable) return;
    let cancelled = false;

    const measureContentWidth = () => {
      if (cancelled) return;
      body.style.removeProperty("--diff-split-content-width");
      const width = Math.ceil(Math.max(
        leftTable.scrollWidth,
        rightTable.scrollWidth,
        leftPane.clientWidth,
        rightPane.clientWidth,
      ));
      body.style.setProperty("--diff-split-content-width", `${width}px`);
    };

    measureContentWidth();
    window.addEventListener("resize", measureContentWidth);
    void document.fonts?.ready.then(measureContentWidth);
    return () => {
      cancelled = true;
      window.removeEventListener("resize", measureContentWidth);
    };
  }, [mode, result]);

  const renderSplitRows = (side: "left" | "right") => result?.rows.map((row, index) => {
    const blockIndex = changeBlockByRow.get(index);
    const active = blockIndex !== undefined && blockIndex === activeChangeIndex;
    const activeBlockStart = active && changeBlockByRow.get(index - 1) !== blockIndex;
    const activeBlockEnd = active && changeBlockByRow.get(index + 1) !== blockIndex;
    return (
      <tr
        key={index}
        className={diffRowClassName(row.kind, active, activeBlockStart, activeBlockEnd)}
        data-diff-row={index}
        data-diff-side={side}
      >
        <td className="diff-table__line">{side === "left" ? row.leftLine ?? "" : row.rightLine ?? ""}</td>
        <td className="diff-table__code">{lineText(side === "left" ? row.leftText : row.rightText)}</td>
      </tr>
    );
  });

  return (
    <div
      className="diff-viewer"
      data-testid="diff-viewer"
      data-color-scheme={colorScheme.id}
      style={{ colorScheme: colorScheme.mode }}
    >
      <header className="diff-viewer__toolbar">
        <div className="diff-viewer__titles">
          <div className="diff-viewer__title">Compare</div>
          {result && (
            <div className="diff-viewer__subtitle">
              {shortPath(result.left.path)} <span>vs</span> {shortPath(result.right.path)}
            </div>
          )}
        </div>
        <div className="diff-viewer__actions">
          <div className="diff-viewer__jump" aria-label="Change navigation">
            <button
              className="diff-viewer__jump-btn"
              type="button"
              onClick={() => jumpChange(-1)}
              disabled={changedBlocks.length === 0}
            >
              Prev
            </button>
            <span className="diff-viewer__jump-count">
              {changedBlocks.length === 0 ? "0/0" : `${activeChangeIndex + 1}/${changedBlocks.length}`}
            </span>
            <button
              className="diff-viewer__jump-btn"
              type="button"
              onClick={() => jumpChange(1)}
              disabled={changedBlocks.length === 0}
            >
              Next
            </button>
          </div>
          <div className="diff-viewer__segmented" role="group" aria-label="Diff view mode">
            <button
              className={mode === "split" ? "diff-viewer__segmented-btn diff-viewer__segmented-btn--active" : "diff-viewer__segmented-btn"}
              type="button"
              onClick={() => setMode("split")}
            >
              Side by side
            </button>
            <button
              className={mode === "unified" ? "diff-viewer__segmented-btn diff-viewer__segmented-btn--active" : "diff-viewer__segmented-btn"}
              type="button"
              onClick={() => setMode("unified")}
            >
              Unified
            </button>
          </div>
          <button className="diff-viewer__close" type="button" onClick={closeWindow}>Close</button>
        </div>
      </header>

      {loading && <main className="diff-viewer__status">Loading diff...</main>}
      {error && <main className="diff-viewer__status diff-viewer__status--error">{error}</main>}
      {result && (
        <>
          <div className="diff-viewer__meta">
            <span>{result.identical ? "No differences" : `${changedCount} changed rows`}</span>
            <span>{rowCount} total rows</span>
          </div>
          {mode === "split" ? (
            <main ref={splitBodyRef} className="diff-viewer__body diff-viewer__body--split">
              <div className="diff-viewer__file-header">
                <span>{result.left.name}</span>
                <span>{result.right.name}</span>
              </div>
              <div className="diff-viewer__split-panes">
                <div
                  ref={leftPaneRef}
                  className="diff-viewer__split-pane"
                  data-testid="diff-split-left"
                  onWheel={(event) => handleSplitWheel(event, rightPaneRef.current)}
                  onScroll={(event) => syncSplitScroll(event.currentTarget, rightPaneRef.current)}
                >
                  <table ref={leftTableRef} className="diff-table diff-table--split">
                    <tbody>{renderSplitRows("left")}</tbody>
                  </table>
                </div>
                <div
                  ref={rightPaneRef}
                  className="diff-viewer__split-pane diff-viewer__split-pane--right"
                  data-testid="diff-split-right"
                  onWheel={(event) => handleSplitWheel(event, leftPaneRef.current)}
                  onScroll={(event) => syncSplitScroll(event.currentTarget, leftPaneRef.current)}
                >
                  <table ref={rightTableRef} className="diff-table diff-table--split">
                    <tbody>{renderSplitRows("right")}</tbody>
                  </table>
                </div>
              </div>
            </main>
          ) : (
            <main ref={unifiedBodyRef} className="diff-viewer__body">
              <table className="diff-table diff-table--unified">
                <tbody>
                  {result.rows.map((row, index) => {
                    const prefix = row.kind === "insert" ? "+" : row.kind === "delete" ? "-" : " ";
                    const text = row.kind === "insert" ? row.rightText : row.leftText;
                    const blockIndex = changeBlockByRow.get(index);
                    const active = blockIndex !== undefined && blockIndex === activeChangeIndex;
                    const activeBlockStart = active && changeBlockByRow.get(index - 1) !== blockIndex;
                    const activeBlockEnd = active && changeBlockByRow.get(index + 1) !== blockIndex;
                    return (
                      <tr
                        key={index}
                        className={diffRowClassName(row.kind, active, activeBlockStart, activeBlockEnd)}
                        data-diff-row={index}
                      >
                        <td className="diff-table__line">{row.leftLine ?? ""}</td>
                        <td className="diff-table__line">{row.rightLine ?? ""}</td>
                        <td className="diff-table__prefix">{prefix}</td>
                        <td className="diff-table__code">{lineText(text)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </main>
          )}
        </>
      )}
    </div>
  );
}
