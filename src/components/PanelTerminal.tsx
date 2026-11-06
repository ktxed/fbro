import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { IconMaximize, IconMinimize, IconExternalLink } from "@tabler/icons-react";
import "@xterm/xterm/css/xterm.css";
import type { WsClient } from "../ws/client";
import type { InteractiveCommand } from "../ws/types";
import { bindingMatches } from "../commands/bindings";
import { formatPrimaryBinding } from "../commands/interactive";
import { DEFAULT_FONT_SIZE } from "../utils/zoom";
import { useColorScheme } from "../theme/useColorScheme";

interface PanelTerminalProps {
  client: WsClient;
  panelId: string;
  /** PTY session id assigned by the BE. Component is not rendered until this is known. */
  sessionId: number;
  /** Registry commands, used for the released chords and the button titles. */
  commands: Record<string, InteractiveCommand>;
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
}

const RESIZE_DEBOUNCE_MS = 50;
/** How long the action toolbar stays up after the terminal opens or toggles. */
const TOOLBAR_IDLE_HIDE_MS = 2500;
/** Grace period after the pointer leaves the hot region before fading out. */
const TOOLBAR_HIDE_DELAY_MS = 400;
/** Extra reach around the toolbar box that reveals it on approach. */
const TOOLBAR_HOT_INSET = 28;
/** Added to the inset when deciding the pointer has left, so travelling to a
 *  button never fades the toolbar out from under the pointer. */
const TOOLBAR_HOT_HYSTERESIS = 24;
const encoder = new TextEncoder();

interface HotBox {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** The two terminal chords the registry owns, matched the way the editor tab
 *  matches released registry bindings: modified keys only, never a plain
 *  printable key, so the shell keeps everything else (Ctrl+M is still a CR). */
function releasedChord(event: KeyboardEvent, command: InteractiveCommand | undefined) {
  return !!command?.bindings.some((binding) => {
    const stroke = binding.strokes[0];
    if (!stroke) return false;
    const special = stroke.ctrl || stroke.meta || stroke.alt || /^F\d{1,2}$/.test(stroke.key);
    return special && bindingMatches(event, binding);
  });
}

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
  commands,
  onClose,
  onBlurToListing,
  onFocusRef,
  fontSize = 18,
  maximized = false,
  onToggleMaximize,
  onPromote,
  solo = false,
}: PanelTerminalProps) {
  const activeScheme = useColorScheme();
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number }>({ cols: 0, rows: 0 });
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Latest maximized + onToggleMaximize, read from inside the xterm key handler
  // (which is bound once on mount and would otherwise close over stale values).
  const maximizedRef = useRef(maximized);
  const onToggleMaximizeRef = useRef(onToggleMaximize);
  const commandsRef = useRef(commands);
  const onPromoteRef = useRef(onPromote);
  useEffect(() => { maximizedRef.current = maximized; }, [maximized]);
  useEffect(() => { onToggleMaximizeRef.current = onToggleMaximize; }, [onToggleMaximize]);
  useEffect(() => { commandsRef.current = commands; }, [commands]);
  useEffect(() => { onPromoteRef.current = onPromote; }, [onPromote]);

  // ── Action-toolbar reveal ──────────────────────────────────────────────────
  // The toolbar floats over the terminal, so it hides terminal output unless the
  // pointer is near it. Transient hover state only: no app state, no persistence.
  const toolbarRef = useRef<HTMLDivElement>(null);
  /** Toolbar box in viewport coordinates, re-measured on layout changes. A
   *  faded toolbar still reports a box because only opacity/visibility change. */
  const hotBoxRef = useRef<HotBox | null>(null);
  const lastPointerRef = useRef<{ x: number; y: number } | null>(null);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [revealed, setRevealed] = useState(true);

  const measureHotBox = useCallback(() => {
    const toolbar = toolbarRef.current;
    if (!toolbar) return;
    const box = toolbar.getBoundingClientRect();
    hotBoxRef.current = { left: box.left, top: box.top, right: box.right, bottom: box.bottom };
  }, []);

  const pointerInHotBox = useCallback((pad: number) => {
    const box = hotBoxRef.current;
    const pointer = lastPointerRef.current;
    if (!box || !pointer) return false;
    return pointer.x >= box.left - pad && pointer.x <= box.right + pad
      && pointer.y >= box.top - pad && pointer.y <= box.bottom + pad;
  }, []);

  const cancelToolbarHide = useCallback(() => {
    if (!hideTimerRef.current) return;
    clearTimeout(hideTimerRef.current);
    hideTimerRef.current = null;
  }, []);

  /** Fade the toolbar out, unless the pointer is parked in the hot region —
   *  which also covers a resize that moves the corner under a still pointer. */
  const scheduleToolbarHide = useCallback((delay: number) => {
    cancelToolbarHide();
    hideTimerRef.current = setTimeout(() => {
      hideTimerRef.current = null;
      if (pointerInHotBox(TOOLBAR_HOT_INSET + TOOLBAR_HOT_HYSTERESIS)) return;
      setRevealed(false);
    }, delay);
  }, [cancelToolbarHide, pointerInHotBox]);

  /** Show the buttons and fade them after the grace window (open, maximize). */
  const revealToolbarForGrace = useCallback(() => {
    measureHotBox();
    setRevealed(true);
    scheduleToolbarHide(TOOLBAR_IDLE_HIDE_MS);
  }, [measureHotBox, scheduleToolbarHide]);

  const rememberPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    lastPointerRef.current = { x: event.clientX, y: event.clientY };
  };

  const onRootPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    rememberPointer(event);
    // Approaching the corner reveals instantly; anywhere else fades out after
    // the leave delay. The wider exit box avoids flicker on the way to a button.
    if (pointerInHotBox(TOOLBAR_HOT_INSET)) {
      cancelToolbarHide();
      setRevealed(true);
    } else {
      scheduleToolbarHide(TOOLBAR_HIDE_DELAY_MS);
    }
  };

  const onRootPointerLeave = (event: ReactPointerEvent<HTMLDivElement>) => {
    rememberPointer(event);
    scheduleToolbarHide(TOOLBAR_HIDE_DELAY_MS);
  };

  // Touch and pen produce no hover: the tap that lands near the corner brings
  // the buttons up, and the next tap can hit them.
  const onRootPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "mouse") return;
    rememberPointer(event);
    if (pointerInHotBox(TOOLBAR_HOT_INSET)) revealToolbarForGrace();
  };

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
      const scheme = activeScheme;
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

    // (Re)bind Esc and the two released registry chords — the handler closes
    // over refs so reattachment is idempotent. Everything else goes to the PTY.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === "keydown") {
        const registry = commandsRef.current;
        if (releasedChord(e, registry["panel.terminal.maximize"])) {
          onToggleMaximizeRef.current?.();
          return false;
        }
        if (releasedChord(e, registry["panel.terminal.promote"])) {
          onPromoteRef.current?.();
          return false;
        }
      }
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

    // Opening (or remounting after a tab switch) shows the buttons long enough
    // to be discovered, then fades them out.
    revealToolbarForGrace();

    const observer = new ResizeObserver(() => {
      // Every layout change — split, maximize, zoom, window resize — moves the
      // corner, so re-read the box the pointer is compared against.
      measureHotBox();
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
      cancelToolbarHide();
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
    const scheme = activeScheme;
    term.options.theme = {
      background: scheme.terminal.background,
      foreground: scheme.terminal.foreground,
      cursor: scheme.terminal.cursor,
      selectionBackground: scheme.terminal.selectionBackground,
    };
  }, [activeScheme.terminal.background, activeScheme.terminal.foreground, activeScheme.terminal.cursor, activeScheme.terminal.selectionBackground]); // eslint-disable-line react-hooks/exhaustive-deps

  // Re-fit on maximize/restore. ResizeObserver also fires, but kicking fit on
  // the next frame avoids a brief mis-sized cell grid during the CSS swap.
  useEffect(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term || !fit) return;
    // Maximized, the toolbar sits in the window corner with nothing around it to
    // approach from, so bring it up for the grace window: restore must be visible.
    revealToolbarForGrace();
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

  const maximizeKey = formatPrimaryBinding(commands["panel.terminal.maximize"]);
  const promoteKey = formatPrimaryBinding(commands["panel.terminal.promote"]);

  return (
    <div
      className={`panel-terminal${solo ? " panel-terminal--solo" : ""}${maximized ? " panel-terminal--maximized" : ""}`}
      onPointerMove={onRootPointerMove}
      onPointerLeave={onRootPointerLeave}
      onPointerDown={onRootPointerDown}
    >
      {/* One opaque toolbar, so terminal text underneath never shows through.
          It fades out unless the pointer approaches the corner or a button holds
          focus; `--revealed` and :focus-within are what bring it back (styles.css). */}
      <div
        ref={toolbarRef}
        className={`panel-terminal__toolbar${revealed ? " panel-terminal__toolbar--revealed" : ""}`}
        role="toolbar"
        aria-label="Terminal actions"
        data-testid="terminal-toolbar"
      >
      {onPromote && (
        <button
          className="panel-terminal__promote"
          title={["Promote terminal to its own tab", promoteKey].filter(Boolean).join(" · ")}
          aria-label="Promote terminal to its own tab"
          aria-keyshortcuts={promoteKey || undefined}
          onClick={(e) => {
            e.stopPropagation();
            onPromote();
          }}
        >
          <IconExternalLink size={12} aria-hidden />
        </button>
      )}
      {onToggleMaximize && (
        <button
          className="panel-terminal__maximize"
          title={[maximized ? "Restore terminal" : "Maximize terminal", maximizeKey].filter(Boolean).join(" · ")}
          aria-label={maximized ? "Restore terminal" : "Maximize terminal"}
          aria-keyshortcuts={maximizeKey || undefined}
          aria-pressed={maximized}
          onClick={(e) => {
            e.stopPropagation();
            onToggleMaximize();
          }}
        >
          {maximized ? <IconMinimize size={12} aria-hidden /> : <IconMaximize size={12} aria-hidden />}
        </button>
      )}
      <button
        className="panel-terminal__close"
        title="Close terminal"
        aria-label="Close terminal"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        ×
      </button>
      </div>
      <div ref={containerRef} className="panel-terminal__xterm" />
    </div>
  );
}
