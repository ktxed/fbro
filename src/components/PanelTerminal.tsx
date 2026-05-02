import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { IconMaximize, IconMinimize } from "@tabler/icons-react";
import "@xterm/xterm/css/xterm.css";
import type { WsClient } from "../ws/client";

interface PanelTerminalProps {
  client: WsClient;
  panelId: string;
  /** PTY session id assigned by the BE. Component is not rendered until this is known. */
  sessionId: number;
  /** Called when the user wants to close the terminal (× button). */
  onClose: () => void;
  /** Called on Escape inside the terminal — used to move keyboard focus back to the listing. */
  onBlurToListing: () => void;
  /** Called with a focus function when the terminal is ready, and with null on unmount. */
  onFocusRef?: (fn: (() => void) | null) => void;
  fontSize?: number;
  /** When true, terminal is maximized to fill the app viewable area. */
  maximized?: boolean;
  /** Toggle maximized/restored state. */
  onToggleMaximize?: () => void;
}

const RESIZE_DEBOUNCE_MS = 50;
const encoder = new TextEncoder();

export function PanelTerminal({
  client,
  panelId,
  sessionId,
  onClose,
  onBlurToListing,
  onFocusRef,
  fontSize = 18,
  maximized = false,
  onToggleMaximize,
}: PanelTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number }>({ cols: 0, rows: 0 });
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Latest maximized + onToggleMaximize, read from inside the xterm key handler
  // (which is bound once on mount and would otherwise close over stale values).
  const maximizedRef = useRef(maximized);
  const onToggleMaximizeRef = useRef(onToggleMaximize);
  useEffect(() => { maximizedRef.current = maximized; }, [maximized]);
  useEffect(() => { onToggleMaximizeRef.current = onToggleMaximize; }, [onToggleMaximize]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      cursorBlink: true,
      fontFamily: "var(--font-mono, monospace)",
      fontSize,
      lineHeight: 1.5,
      letterSpacing: 1.5,
      scrollback: 5000,
      theme: {
        background: "#1e1e1e",
        foreground: "#dddddd",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    termRef.current = term;
    fitRef.current = fit;

    // Initial fit + send dimensions so PTY matches the visual cell grid.
    try {
      fit.fit();
    } catch {
      // ignore
    }
    const initialCols = term.cols;
    const initialRows = term.rows;
    lastSizeRef.current = { cols: initialCols, rows: initialRows };
    client.sendIntent("ResizeTerminal", panelId, { cols: initialCols, rows: initialRows });

    // Wire data → BE binary write.
    const dataDisposer = term.onData((data) => {
      client.sendTerminalInput(sessionId, encoder.encode(data));
    });

    // Subscribe to PTY output bytes.
    client.subscribeTerminalOutput(sessionId, (bytes) => {
      term.write(bytes);
    });

    // Esc → restore if maximized, otherwise blur to listing. Leaves the session alive.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === "keydown" && e.key === "Escape") {
        if (maximizedRef.current && onToggleMaximizeRef.current) {
          onToggleMaximizeRef.current();
        } else {
          onBlurToListing();
        }
        return false;
      }
      return true;
    });

    // Auto-focus the terminal on mount, and register focus handle with parent.
    term.focus();
    onFocusRef?.(() => term.focus());

    // Resize observer with debounce.
    const observer = new ResizeObserver(() => {
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = setTimeout(() => {
        try {
          fit.fit();
        } catch {
          return;
        }
        const cols = term.cols;
        const rows = term.rows;
        if (cols < 1 || rows < 1) return;
        if (cols === lastSizeRef.current.cols && rows === lastSizeRef.current.rows) return;
        lastSizeRef.current = { cols, rows };
        client.sendIntent("ResizeTerminal", panelId, { cols, rows });
      }, RESIZE_DEBOUNCE_MS);
    });
    observer.observe(container);

    return () => {
      observer.disconnect();
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
      dataDisposer.dispose();
      client.unsubscribeTerminalOutput(sessionId);
      onFocusRef?.(null);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  useEffect(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term || !fit) return;
    term.options.fontSize = fontSize;
    term.options.lineHeight = 1.5;
    term.options.letterSpacing = 1.5;
    try { fit.fit(); } catch { /* ignore */ }
    const cols = term.cols;
    const rows = term.rows;
    if (cols > 0 && rows > 0) {
      lastSizeRef.current = { cols, rows };
      client.sendIntent("ResizeTerminal", panelId, { cols, rows });
    }
  }, [fontSize]); // eslint-disable-line react-hooks/exhaustive-deps

  // Re-fit on maximize/restore. ResizeObserver also fires, but kicking fit on
  // the next frame avoids a brief mis-sized cell grid during the CSS swap.
  useEffect(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term || !fit) return;
    const raf = requestAnimationFrame(() => {
      try { fit.fit(); } catch { /* ignore */ }
      const cols = term.cols;
      const rows = term.rows;
      if (cols > 0 && rows > 0
          && (cols !== lastSizeRef.current.cols || rows !== lastSizeRef.current.rows)) {
        lastSizeRef.current = { cols, rows };
        client.sendIntent("ResizeTerminal", panelId, { cols, rows });
      }
      term.focus();
    });
    return () => cancelAnimationFrame(raf);
  }, [maximized]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className={`panel-terminal${maximized ? " panel-terminal--maximized" : ""}`}>
      {onToggleMaximize && (
        <button
          className="panel-terminal__maximize"
          title={maximized ? "Restore terminal" : "Maximize terminal"}
          onClick={(e) => {
            e.stopPropagation();
            onToggleMaximize();
          }}
        >
          {maximized ? <IconMinimize size={12} /> : <IconMaximize size={12} />}
        </button>
      )}
      <button
        className="panel-terminal__close"
        title="Close terminal"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        ×
      </button>
      <div ref={containerRef} className="panel-terminal__xterm" />
    </div>
  );
}
