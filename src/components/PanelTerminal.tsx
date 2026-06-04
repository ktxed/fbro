import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { IconMaximize, IconMinimize, IconExternalLink } from "@tabler/icons-react";
import "@xterm/xterm/css/xterm.css";
import type { WsClient } from "../ws/client";
import { DEFAULT_FONT_SIZE } from "../utils/zoom";
import { resolveColorScheme } from "../theme/colorSchemes";

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
  /** Promote the embedded terminal split into its own sibling tab. Omitted
   *  when the terminal already lives in its own tab. */
  onPromote?: () => void;
  /** True when the terminal owns the whole panel (terminal tab) rather than
   *  sharing space with a listing split. */
  solo?: boolean;
  colorScheme?: string;
}

const RESIZE_DEBOUNCE_MS = 50;
const encoder = new TextEncoder();

interface CachedTerm {
  term: Terminal;
  fit: FitAddon;
  unsubscribe: () => void;
}

/// Module-level cache so the xterm buffer survives tab switches (which
/// unmount the component). The PTY keeps streaming output into the cached
/// term whether or not the DOM is attached. Disposal is triggered by the
/// store when the BE signals TerminalClosed.
const TERM_CACHE = new Map<number, CachedTerm>();

export function disposeCachedTerminal(sessionId: number) {
  const cached = TERM_CACHE.get(sessionId);
  if (!cached) return;
  cached.unsubscribe();
  cached.term.dispose();
  TERM_CACHE.delete(sessionId);
}

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
  onPromote,
  solo = false,
  colorScheme = "fbro-dark",
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

    let cached = TERM_CACHE.get(sessionId);
    let dataDisposer: { dispose: () => void } | null = null;

    if (cached) {
      // Re-attach the cached xterm DOM to the new container. The PTY output
      // listener stayed subscribed the whole time, so the buffer is current.
      if (cached.term.element && cached.term.element.parentElement !== container) {
        container.appendChild(cached.term.element);
      }
    } else {
      const scheme = resolveColorScheme(colorScheme);
      const term = new Terminal({
        cursorBlink: true,
        fontFamily: '"JetBrains Mono", monospace',
        fontSize,
        lineHeight: 1.1,
        letterSpacing: 0.9 * (fontSize / DEFAULT_FONT_SIZE),
        scrollback: 5000,
        theme: {
          background: scheme.terminal.background,
          foreground: scheme.terminal.foreground,
          cursor: scheme.terminal.cursor,
          selectionBackground: scheme.terminal.selectionBackground,
        },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(container);

      // Wire data → BE binary write. (Survives across mounts via the cache.)
      dataDisposer = term.onData((data) => {
        client.sendTerminalInput(sessionId, encoder.encode(data));
      });

      // Subscribe to PTY output bytes. Stays alive across remounts so the
      // buffer keeps filling while the tab is hidden.
      client.subscribeTerminalOutput(sessionId, (bytes) => {
        term.write(bytes);
      });

      cached = {
        term,
        fit,
        unsubscribe: () => {
          dataDisposer?.dispose();
          client.unsubscribeTerminalOutput(sessionId);
        },
      };
      TERM_CACHE.set(sessionId, cached);
    }

    const term = cached.term;
    const fit = cached.fit;
    termRef.current = term;
    fitRef.current = fit;

    // Re-fit for the current container size.
    try { fit.fit(); } catch { /* ignore */ }
    const initialCols = term.cols;
    const initialRows = term.rows;
    if (initialCols !== lastSizeRef.current.cols || initialRows !== lastSizeRef.current.rows) {
      lastSizeRef.current = { cols: initialCols, rows: initialRows };
      client.sendIntent("ResizeTerminal", panelId, { cols: initialCols, rows: initialRows });
    }

    // (Re)bind Esc — handler closes over refs so reattachment is idempotent.
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

    term.focus();
    onFocusRef?.(() => term.focus());

    const observer = new ResizeObserver(() => {
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = setTimeout(() => {
        try { fit.fit(); } catch { return; }
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
      onFocusRef?.(null);
      // Detach the xterm DOM but keep the Terminal alive in the cache so a
      // future remount restores the buffer. disposeCachedTerminal() (called
      // from the store on TerminalClosed) does the real cleanup.
      const c = TERM_CACHE.get(sessionId);
      if (c?.term.element?.parentElement === container) {
        container.removeChild(c.term.element);
      }
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
    term.options.lineHeight = 1.1;
    term.options.letterSpacing = 0.9 * (fontSize / DEFAULT_FONT_SIZE);
    try { fit.fit(); } catch { /* ignore */ }
    const cols = term.cols;
    const rows = term.rows;
    if (cols > 0 && rows > 0) {
      lastSizeRef.current = { cols, rows };
      client.sendIntent("ResizeTerminal", panelId, { cols, rows });
    }
  }, [fontSize]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    const scheme = resolveColorScheme(colorScheme);
    term.options.theme = {
      background: scheme.terminal.background,
      foreground: scheme.terminal.foreground,
      cursor: scheme.terminal.cursor,
      selectionBackground: scheme.terminal.selectionBackground,
    };
  }, [colorScheme]);

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
    <div
      className={`panel-terminal${solo ? " panel-terminal--solo" : ""}${maximized ? " panel-terminal--maximized" : ""}`}
    >
      {onPromote && (
        <button
          className="panel-terminal__promote"
          title="Promote terminal to its own tab"
          onClick={(e) => {
            e.stopPropagation();
            onPromote();
          }}
        >
          <IconExternalLink size={12} />
        </button>
      )}
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
