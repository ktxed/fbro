import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { resolveWsUrl, WsClient } from "../ws/client";
import type {
  BEMessage,
  CompareDiffReadyMessage,
  CompareDiffResult,
  StatePatchMessage,
  StateSnapshotMessage,
} from "../ws/types";
import { applyZoom, DEFAULT_FONT_SIZE } from "../utils/zoom";
import { DEFAULT_COLOR_SCHEME_ID, resolveColorScheme, type ColorSchemeId } from "../theme/colorSchemes";

type DiffViewMode = "split" | "unified";

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
  return value === "" ? "\u00A0" : value ?? "";
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
      if (event.key === "n" || event.key === "j" || event.key === "F7") {
        event.preventDefault();
        jumpChange(1);
      }
      if (event.key === "p" || event.key === "k" || event.key === "F6") {
        event.preventDefault();
        jumpChange(-1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [jumpChange]);

  const colorScheme = resolveColorScheme(colorSchemeId);
  const rowCount = result?.rows.length ?? 0;
  const changedCount = useMemo(
    () => result?.rows.filter((row) => row.kind !== "equal").length ?? 0,
    [result],
  );

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
            <main className="diff-viewer__body diff-viewer__body--split">
              <div className="diff-viewer__file-header">
                <span>{result.left.name}</span>
                <span>{result.right.name}</span>
              </div>
              <table className="diff-table diff-table--split">
                <tbody>
                  {result.rows.map((row, index) => {
                    const blockIndex = changeBlockByRow.get(index);
                    const active = blockIndex !== undefined && blockIndex === activeChangeIndex;
                    return (
                      <tr
                        key={index}
                        className={`diff-table__row diff-table__row--${row.kind}${active ? " diff-table__row--active-change" : ""}`}
                        data-diff-row={index}
                      >
                        <td className="diff-table__line">{row.leftLine ?? ""}</td>
                        <td className="diff-table__code">{lineText(row.leftText)}</td>
                        <td className="diff-table__line">{row.rightLine ?? ""}</td>
                        <td className="diff-table__code">{lineText(row.rightText)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </main>
          ) : (
            <main className="diff-viewer__body">
              <table className="diff-table diff-table--unified">
                <tbody>
                  {result.rows.map((row, index) => {
                    const prefix = row.kind === "insert" ? "+" : row.kind === "delete" ? "-" : " ";
                    const text = row.kind === "insert" ? row.rightText : row.leftText;
                    const blockIndex = changeBlockByRow.get(index);
                    const active = blockIndex !== undefined && blockIndex === activeChangeIndex;
                    return (
                      <tr
                        key={index}
                        className={`diff-table__row diff-table__row--${row.kind}${active ? " diff-table__row--active-change" : ""}`}
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
