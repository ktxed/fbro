import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { IconAlertTriangle, IconEye, IconMaximize, IconMinimize, IconX } from "@tabler/icons-react";
import type { Dispatch } from "react";
import type { AppAction } from "../store";
import type { WsClient } from "../ws/client";
import type { EditorTabInfo, InteractiveCommand, PanelState } from "../ws/types";
import { formatPrimaryBinding } from "../commands/interactive";
import { bindingMatches, formatBinding } from "../commands/bindings";
import { isMac } from "../utils/platform";
import {
  attachView,
  detachView,
  editorStatus,
  editorStatusVersion,
  ensureSession,
  flushEditor,
  getSession,
  markLoaded,
  publishEditorCursor,
  noteUserChange,
  onExternalChange,
  setPreviewing,
  subscribeEditorStatus,
} from "../editor/sessions";

type CodeMirrorModule = typeof import("../editor/codemirror");
let codeMirrorModule: Promise<CodeMirrorModule> | null = null;
function loadCodeMirror() {
  codeMirrorModule ??= import("../editor/codemirror");
  return codeMirrorModule;
}

/** Mod-s is an editor-local shortcut, not a registry command. */
const SAVE_KEY_LABEL = formatBinding({ strokes: [{ key: "s", ctrl: !isMac, meta: isMac }] });
/** Mod-Shift-m, also editor-local. */
const MAXIMIZE_KEY_LABEL = formatBinding({ strokes: [{ key: "m", ctrl: !isMac, meta: isMac, shift: true }] });

interface EditorActions {
  escape: () => void;
  toggleMaximize: () => void;
  save: () => void;
  close: () => void;
  /** Conflict resolution: take the disk version. */
  reload: () => void;
  /** Conflict resolution: save the local text as a sibling copy, then reload. */
  keepBoth: () => void;
  /** Publish the cursor line for quick preview. */
  cursor: (view: import("../editor/codemirror").EditorView) => void;
}

/// The cached CodeMirror state outlives a mounted component, so its key
/// handlers look up the current component's actions by session id.
const ACTIONS = new Map<number, EditorActions>();

/** Run an editor action from outside the editor (the conflict dialog). */
export function runEditorAction(sessionId: number, action: "reload" | "keepBoth") {
  ACTIONS.get(sessionId)?.[action]();
}
function handlersFor(sessionId: number) {
  return {
    onEscape: () => ACTIONS.get(sessionId)?.escape(),
    onSave: () => ACTIONS.get(sessionId)?.save(),
    onClose: () => ACTIONS.get(sessionId)?.close(),
    onToggleMaximize: () => ACTIONS.get(sessionId)?.toggleMaximize(),
    onUserChange: () => noteUserChange(sessionId),
    onCursor: (view: import("../editor/codemirror").EditorView) => ACTIONS.get(sessionId)?.cursor(view),
  };
}

/** The first line, capped, for `#!` detection. */
function firstLine(text: string) {
  const end = text.indexOf("\n");
  return (end === -1 ? text : text.slice(0, end)).replace(/^﻿/, "").slice(0, 256);
}

/** Registry bindings that may leave the editor: modified keys or F-keys only,
 *  never a plain printable key (macOS binds F3's command to plain "3"). */
function passThrough(command: InteractiveCommand | undefined, event: KeyboardEvent) {
  return !!command?.bindings.some((binding) => {
    const stroke = binding.strokes[0];
    if (!stroke) return false;
    const special = stroke.ctrl || stroke.meta || stroke.alt || /^F\d{1,2}$/.test(stroke.key);
    return special && bindingMatches(event.nativeEvent, binding);
  });
}

/** Registry commands the editor hands to the app unchanged. */
const RELEASED_COMMAND_IDS = ["panel.tab.next", "panel.tab.prev", "app.command_palette"] as const;

interface PanelEditorProps {
  panelId: string;
  panel: PanelState & { editor: EditorTabInfo };
  client: WsClient;
  dispatch: Dispatch<AppAction>;
  commands: Record<string, InteractiveCommand>;
  focused: boolean;
  /** A modal covers the app: the editor is inert. */
  blocked: boolean;
  /** Quick preview is showing this editor's file. */
  previewing: boolean;
  /** This editor fills the window. */
  maximized: boolean;
  /** Run a registry command released by the editor (tab cycling, palette)
   *  with this tab as the focused panel. */
  onRunCommand: (commandId: string) => void;
}

/** Built-in editor tab: CodeMirror over a backend editor session. */
export function PanelEditor({ panelId, panel, client, dispatch, commands, focused, blocked, previewing, maximized, onRunCommand }: PanelEditorProps) {
  const { sessionId, fileName, filePath } = panel.editor;
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<import("../editor/codemirror").EditorView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  /** Set once CodeMirror is loaded: (re)load text into the editor. */
  const loadRef = useRef<((intent: "ReloadEditorSession" | "KeepBothEditorVersions") => void) | null>(null);
  useSyncExternalStore(subscribeEditorStatus, editorStatusVersion);
  const status = editorStatus(sessionId);

  // Escape / Mod-s / Mod-w as seen by CodeMirror's keymap.
  const latest = useRef({ panelId, panel, blocked, status, maximized });
  latest.current = { panelId, panel, blocked, status, maximized };
  useEffect(() => {
    const openConflict = () => {
      const problem = editorStatus(sessionId).problem;
      if (problem !== "conflict" && problem !== "missing") return false;
      dispatch({ type: "OPEN_EDITOR_CONFLICT", conflict: { panelId, sessionId, fileName, kind: problem } });
      return true;
    };
    ACTIONS.set(sessionId, {
      toggleMaximize: () => {
        if (latest.current.blocked) return;
        dispatch({ type: "SET_MAXIMIZED_EDITOR", panelId: latest.current.maximized ? null : panelId });
      },
      escape: () => {
        if (latest.current.blocked) return;
        // Like a maximized terminal: Escape first restores the layout.
        if (latest.current.maximized) {
          dispatch({ type: "SET_MAXIMIZED_EDITOR", panelId: null });
          return;
        }
        void flushEditor(sessionId);
        client.sendIntent("ReturnToOriginTab", undefined, { panelId });
      },
      save: () => {
        if (latest.current.blocked || openConflict()) return;
        void flushEditor(sessionId);
      },
      close: () => {
        if (latest.current.blocked) return;
        void flushEditor(sessionId).then(() => client.sendIntent("panel.tab.close", undefined, { panelId }));
      },
      reload: () => loadRef.current?.("ReloadEditorSession"),
      keepBoth: () => loadRef.current?.("KeepBothEditorVersions"),
      cursor: (view) => view.requestMeasure({
        key: "fbro-cursor-line",
        read: (v) => {
          const head = v.state.selection.main.head;
          const coords = v.coordsAtPos(head);
          const box = v.scrollDOM.getBoundingClientRect();
          const fraction = coords && box.height > 0 ? (coords.top - box.top) / box.height : 0.3;
          return { line: v.state.doc.lineAt(head).number, fraction: Math.min(1, Math.max(0, fraction)) };
        },
        write: (cursor) => publishEditorCursor(filePath, cursor),
      }),
    });
    return () => { ACTIONS.delete(sessionId); };
  }, [client, dispatch, fileName, filePath, panelId, sessionId]);

  useEffect(() => {
    setPreviewing(sessionId, previewing);
  }, [previewing, sessionId]);

  // Mount: reuse the cached state, or load the file from the backend.
  useEffect(() => {
    let disposed = false;
    /** Text that arrived while the first load waits for its language. */
    let pendingText: string | null = null;
    const session = ensureSession(sessionId, panelId);
    setLoadError(null);
    setReady(false);

    const mount = (cm: CodeMirrorModule, state: import("../editor/codemirror").EditorState) => {
      if (disposed || !hostRef.current) return;
      const view = cm.createEditorView(hostRef.current, state);
      viewRef.current = view;
      attachView(sessionId, () => view.state.doc.toString());
      setReady(true);
    };

    const requestLoad = (
      cm: CodeMirrorModule,
      intent: "LoadEditorSession" | "ReloadEditorSession" | "KeepBothEditorVersions",
    ) => {
      const requestId = crypto.randomUUID();
      const unsubscribe = client.subscribeMessages((message) => {
        if (message.type === "EditorSessionFailed" && message.requestId === requestId) {
          unsubscribe();
          if (!disposed) setLoadError(message.message);
        }
        // Keep both failed to write the copy: the conflict stays open.
        if (message.type === "SaveEditorFailed" && message.requestId === requestId) unsubscribe();
        if (message.type !== "EditorSessionReady" || message.requestId !== requestId) return;
        unsubscribe();
        if (disposed) return;
        const view = viewRef.current;
        if (view) {
          cm.replaceFromDisk(view, message.text);
          markLoaded(sessionId);
          return;
        }
        // First load: the language (a lazy chunk) is ready before the view
        // mounts, so the text never shows unhighlighted. A reload that lands
        // meanwhile only replaces the text.
        const loading = pendingText !== null;
        pendingText = message.text;
        if (loading) return;
        void cm.loadLanguage(latest.current.panel.editor.fileName, firstLine(message.text)).then((language) => {
          const text = pendingText ?? "";
          pendingText = null;
          if (disposed) return;
          mount(cm, cm.createEditorState(text, handlersFor(sessionId), language));
          markLoaded(sessionId);
        });
      });
      const text = intent === "KeepBothEditorVersions" ? viewRef.current?.state.doc.toString() : undefined;
      client.sendIntent(intent, undefined, { panelId, requestId, ...(text === undefined ? {} : { text }) });
      return unsubscribe;
    };

    let cancelLoad: (() => void) | undefined;
    let stopExternal: (() => void) | undefined;
    void loadCodeMirror().then((cm) => {
      if (disposed) return;
      if (session.cached && !session.reloadOnMount) mount(cm, session.cached);
      else cancelLoad = requestLoad(cm, "LoadEditorSession");
      loadRef.current = (intent) => {
        cancelLoad?.();
        cancelLoad = requestLoad(cm, intent);
      };
      // A fully saved buffer follows external changes.
      stopExternal = onExternalChange(sessionId, () => {
        cancelLoad?.();
        cancelLoad = requestLoad(cm, "ReloadEditorSession");
      });
    });

    return () => {
      disposed = true;
      loadRef.current = null;
      cancelLoad?.();
      stopExternal?.();
      const view = viewRef.current;
      viewRef.current = null;
      // Leaving the tab saves; the state (text, undo history) stays cached.
      if (getSession(sessionId)) {
        detachView(sessionId, view?.state ?? null);
        void flushEditor(sessionId);
      }
      view?.destroy();
    };
  }, [client, panelId, sessionId]);

  // Keyboard focus follows the backend's focus onto this tab.
  useEffect(() => {
    if (!ready || !focused || blocked) return;
    const view = viewRef.current;
    if (view && !view.hasFocus) view.focus();
  }, [ready, focused, blocked]);

  // Leaving the tab (switch, close) ends full-screen mode.
  useEffect(() => () => {
    if (latest.current.maximized) dispatch({ type: "SET_MAXIMIZED_EDITOR", panelId: null });
  }, [dispatch]);

  // Keep the cursor in view when the editor changes size.
  useEffect(() => {
    viewRef.current?.requestMeasure();
    if (!blocked) viewRef.current?.focus();
  }, [maximized]); // eslint-disable-line react-hooks/exhaustive-deps

  // Window blur saves too.
  useEffect(() => {
    const onBlur = () => { void flushEditor(sessionId); };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [sessionId]);

  const openViewer = () => {
    void flushEditor(sessionId).then(() => client.sendIntent("ViewFile", undefined, {
      panelId,
      path: panel.path,
      sourceId: panel.sourceId,
      filename: fileName,
    }));
  };
  const togglePreview = () => {
    void flushEditor(sessionId);
    // The preview lives in the other panel, which full screen would cover.
    if (maximized) dispatch({ type: "SET_MAXIMIZED_EDITOR", panelId: null });
    dispatch({ type: "TOGGLE_EMBEDDED_VIEWER" });
  };
  const close = () => ACTIONS.get(sessionId)?.close();

  // Everything typed here belongs to the editor. Only a few registry
  // commands leave it, and nothing reaches the listing or global shortcuts.
  const onKeyDown = (event: KeyboardEvent) => {
    event.stopPropagation();
    if (event.defaultPrevented || blocked) return;
    if (passThrough(commands["file.view"], event)) {
      event.preventDefault();
      openViewer();
    } else if (passThrough(commands["panel.viewer.embed"], event)) {
      event.preventDefault();
      togglePreview();
    } else if (passThrough(commands["notes.new"], event)) {
      // Another note from inside an editor; it opens next to this tab.
      event.preventDefault();
      void flushEditor(sessionId);
      const requestId = crypto.randomUUID();
      dispatch({ type: "SET_PENDING_NOTE", note: { requestId } });
      client.sendIntent("CreateNote", undefined, { panelId, requestId });
    } else {
      // Tab cycling and the palette: Ctrl+Tab, Ctrl+Shift+Tab and
      // Ctrl/Cmd+Shift+P mean nothing to CodeMirror. Leaving the tab saves
      // on unmount; flushing first just starts the save sooner.
      const released = RELEASED_COMMAND_IDS.find((id) => passThrough(commands[id], event));
      if (!released) return;
      event.preventDefault();
      if (released !== "app.command_palette") void flushEditor(sessionId);
      onRunCommand(released);
    }
  };

  const conflict = status.problem === "conflict" || status.problem === "missing";
  const saveKey = SAVE_KEY_LABEL;
  const previewKey = formatPrimaryBinding(commands["panel.viewer.embed"]);

  return (
    <div
      className={`panel-editor${maximized ? " panel-editor--maximized" : ""}`}
      data-testid={`panel-editor-${panelId}`}
      data-maximized={maximized || undefined}
      data-editor-session={sessionId}
      onKeyDown={onKeyDown}
    >
      <div className="panel-editor__header">
        <span className="panel-editor__name" title={filePath}>{fileName}</span>
        {status.pending && !status.problem && (
          <span className="panel-editor__state" data-testid="editor-saving">Saving…</span>
        )}
        <button
          type="button"
          className="panel-editor__btn"
          title={`${maximized ? "Restore" : "Full screen"} · ${MAXIMIZE_KEY_LABEL}`}
          aria-label={maximized ? "Restore editor" : "Full screen editor"}
          aria-keyshortcuts={MAXIMIZE_KEY_LABEL}
          aria-pressed={maximized}
          data-testid="editor-maximize"
          tabIndex={-1}
          disabled={blocked}
          onClick={() => ACTIONS.get(sessionId)?.toggleMaximize()}
        >
          {maximized ? <IconMinimize size={15} aria-hidden /> : <IconMaximize size={15} aria-hidden />}
        </button>
        <button
          type="button"
          className="panel-editor__btn"
          title={["Preview", previewKey].filter(Boolean).join(" · ")}
          aria-label="Toggle preview"
          aria-keyshortcuts={previewKey || undefined}
          data-command-id="panel.viewer.embed"
          tabIndex={-1}
          disabled={blocked}
          onClick={togglePreview}
        >
          <IconEye size={15} aria-hidden />
        </button>
        <button
          type="button"
          className="panel-editor__btn"
          title={`Close · ${formatPrimaryBinding(commands["panel.tab.close"])}`}
          aria-label="Close editor tab"
          data-testid="editor-close"
          tabIndex={-1}
          disabled={blocked}
          onClick={close}
        >
          <IconX size={15} aria-hidden />
        </button>
      </div>
      {(conflict || status.problem === "error") && (
        <div className="panel-editor__bar" role="status" data-testid="editor-conflict-bar">
          <IconAlertTriangle size={14} aria-hidden />
          <span>
            {conflict
              ? `${status.message} Autosave is paused. Press ${saveKey} to resolve.`
              : `Could not save: ${status.message} Autosave retries after your next change.`}
          </span>
          {conflict && (
            <button
              type="button"
              className="panel-editor__bar-btn"
              aria-keyshortcuts={saveKey}
              data-testid="editor-resolve"
              disabled={blocked}
              onClick={() => ACTIONS.get(sessionId)?.save()}
            >
              Resolve…
            </button>
          )}
        </div>
      )}
      {loadError ? (
        <div className="panel-editor__error" role="alert">
          {loadError}
        </div>
      ) : (
        <div
          className="panel-editor__host"
          ref={hostRef}
          aria-busy={!ready}
          // Inert under a modal: no clicks or typing reach the editor.
          inert={blocked || undefined}
        />
      )}
    </div>
  );
}
