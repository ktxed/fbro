import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
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
}

const RESIZE_DEBOUNCE_MS = 50;
const encoder = new TextEncoder();

export function PanelTerminal({
  client,
  panelId,
  sessionId,
  onClose,
  onBlurToListing,
}: PanelTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number }>({ cols: 0, rows: 0 });
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      cursorBlink: true,
      fontFamily: "var(--font-mono, monospace)",
      fontSize: 13,
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

    // Esc → blur terminal, focus listing. Leaves the session alive.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === "keydown" && e.key === "Escape") {
        onBlurToListing();
        return false;
      }
      return true;
    });

    // Auto-focus the terminal on mount.
    term.focus();

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
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return (
    <div className="panel-terminal">
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
