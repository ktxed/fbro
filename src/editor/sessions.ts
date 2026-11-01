// Autosave controller for built-in editor tabs.
//
// One `EditorSession` per editor tab, keyed by the backend's `sessionId`. It
// lives at module level (like the terminal's TERM_CACHE) so the CodeMirror
// state — text, undo history, selection — survives tab switches, and saves
// keep their bookkeeping while a tab is not mounted. Only the frontend knows
// whether the text differs from disk; the backend owns disk state, conflict
// checks, and every write. Nothing here imports CodeMirror at runtime.
import type { EditorState } from "@codemirror/state";
import type { WsClient } from "../ws/client";
import type { BEMessage } from "../ws/types";

/** 1 s after the last change; shorter while quick preview shows the file. */
export const AUTOSAVE_DELAY_MS = 1000;
export const AUTOSAVE_PREVIEW_DELAY_MS = 400;
/** How long quitting waits for pending saves. */
const QUIT_FLUSH_TIMEOUT_MS = 2000;

export type EditorProblem = "conflict" | "missing" | "error";

export interface EditorStatus {
  /** A save is scheduled or in flight. */
  pending: boolean;
  /** Autosave is paused (conflict/missing) or the last save failed (error). */
  problem: EditorProblem | null;
  message: string | null;
}

interface Session {
  sessionId: number;
  panelId: string;
  /** Cached CodeMirror state while the tab is not mounted. */
  cached: EditorState | null;
  /** Current text, from the live view when mounted. */
  readText: (() => string) | null;
  revision: number;
  savedRevision: number;
  inFlight: { requestId: string; revision: number } | null;
  timer: ReturnType<typeof setTimeout> | null;
  previewing: boolean;
  problem: EditorProblem | null;
  message: string | null;
  /** External change seen while not mounted and clean: reload on mount. */
  reloadOnMount: boolean;
  waiters: Array<() => void>;
}

const sessions = new Map<number, Session>();
const listeners = new Set<() => void>();
let client: WsClient | null = null;
let statusVersion = 0;

function notify() {
  statusVersion += 1;
  for (const listener of listeners) listener();
}

export function subscribeEditorStatus(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function editorStatusVersion(): number {
  return statusVersion;
}

export function editorStatus(sessionId: number | undefined): EditorStatus {
  const session = sessionId === undefined ? undefined : sessions.get(sessionId);
  if (!session) return { pending: false, problem: null, message: null };
  return {
    pending: !!session.inFlight || session.timer !== null || isDirty(session),
    problem: session.problem,
    message: session.message,
  };
}

function isDirty(session: Session) {
  return session.revision !== session.savedRevision;
}

function isIdle(session: Session) {
  return !session.inFlight && (!isDirty(session) || session.problem !== null);
}

function settle(session: Session) {
  if (!isIdle(session)) return;
  const waiters = session.waiters.splice(0);
  for (const resolve of waiters) resolve();
}

export function getSession(sessionId: number) {
  return sessions.get(sessionId);
}

export function ensureSession(sessionId: number, panelId: string): Session {
  let session = sessions.get(sessionId);
  if (!session) {
    session = {
      sessionId,
      panelId,
      cached: null,
      readText: null,
      revision: 0,
      savedRevision: 0,
      inFlight: null,
      timer: null,
      previewing: false,
      problem: null,
      message: null,
      reloadOnMount: false,
      waiters: [],
    };
    sessions.set(sessionId, session);
  }
  session.panelId = panelId;
  return session;
}

/** Mounted view: text comes from it. Unmounting caches its state. */
export function attachView(sessionId: number, readText: () => string) {
  const session = sessions.get(sessionId);
  if (session) session.readText = readText;
}

export function detachView(sessionId: number, state: EditorState | null) {
  const session = sessions.get(sessionId);
  if (!session) return;
  session.cached = state;
  session.readText = state ? () => state.doc.toString() : null;
}

/** The tab closed: forget everything about it. */
export function disposeEditorSession(sessionId: number) {
  const session = sessions.get(sessionId);
  if (!session) return;
  if (session.timer) clearTimeout(session.timer);
  for (const resolve of session.waiters.splice(0)) resolve();
  sessions.delete(sessionId);
  notify();
}

export function setPreviewing(sessionId: number, previewing: boolean) {
  const session = sessions.get(sessionId);
  if (session) session.previewing = previewing;
}

/** Text was (re)loaded from disk: the buffer is clean. */
export function markLoaded(sessionId: number) {
  const session = sessions.get(sessionId);
  if (!session) return;
  session.savedRevision = session.revision;
  session.problem = null;
  session.message = null;
  session.reloadOnMount = false;
  settle(session);
  notify();
}

export function noteUserChange(sessionId: number) {
  const session = sessions.get(sessionId);
  if (!session) return;
  session.revision += 1;
  if (session.problem === "error") {
    // A failed save is retried by the next change.
    session.problem = null;
    session.message = null;
  }
  if (session.problem === null) schedule(session);
  notify();
}

function schedule(session: Session) {
  if (session.timer) clearTimeout(session.timer);
  const delay = session.previewing ? AUTOSAVE_PREVIEW_DELAY_MS : AUTOSAVE_DELAY_MS;
  session.timer = setTimeout(() => {
    session.timer = null;
    void flushEditor(session.sessionId);
  }, delay);
}

/** Save now if there is anything to save. Resolves when the session is idle. */
export function flushEditor(sessionId: number, force = false): Promise<void> {
  const session = sessions.get(sessionId);
  if (!session) return Promise.resolve();
  if (session.timer) {
    clearTimeout(session.timer);
    session.timer = null;
  }
  const done = new Promise<void>((resolve) => session.waiters.push(resolve));
  if (session.inFlight) {
    // The reply schedules the follow-up save for newer changes.
    return done;
  }
  if ((session.problem !== null && session.problem !== "error" && !force) || !session.readText || !client) {
    settle(session);
    return done;
  }
  if (!isDirty(session) && !force) {
    settle(session);
    return done;
  }
  const requestId = crypto.randomUUID();
  session.inFlight = { requestId, revision: session.revision };
  client.sendIntent("SaveEditorFile", undefined, {
    panelId: session.panelId,
    requestId,
    revision: session.revision,
    text: session.readText(),
    force,
  });
  notify();
  return done;
}

/** Quitting: save every session, then report how many could not be saved. */
export async function flushAllEditors(): Promise<number> {
  const all = [...sessions.values()];
  const flushes = Promise.all(all.map((session) => flushEditor(session.sessionId)));
  await Promise.race([flushes, new Promise((resolve) => setTimeout(resolve, QUIT_FLUSH_TIMEOUT_MS))]);
  return all.filter((session) => isDirty(session) || session.inFlight).length;
}

function sessionForPanel(panelId: string): Session | undefined {
  for (const session of sessions.values()) {
    if (session.panelId === panelId) return session;
  }
  return undefined;
}

/** Callbacks a mounted editor registers to react to disk changes. */
type ExternalHandler = (kind: "reload") => void;
const externalHandlers = new Map<number, ExternalHandler>();

export function onExternalChange(sessionId: number, handler: ExternalHandler): () => void {
  externalHandlers.set(sessionId, handler);
  return () => {
    if (externalHandlers.get(sessionId) === handler) externalHandlers.delete(sessionId);
  };
}

function handleMessage(message: BEMessage) {
  switch (message.type) {
    case "SaveEditorOk": {
      const session = sessionForPanel(message.panelId);
      if (!session || session.inFlight?.requestId !== message.requestId) return;
      session.inFlight = null;
      session.savedRevision = Math.max(session.savedRevision, message.revision);
      if (session.problem !== "error" || !isDirty(session)) {
        session.problem = null;
        session.message = null;
      }
      if (isDirty(session) && session.problem === null) void flushEditor(session.sessionId);
      settle(session);
      notify();
      return;
    }
    case "SaveEditorFailed": {
      const session = sessionForPanel(message.panelId);
      if (!session || session.inFlight?.requestId !== message.requestId) return;
      session.inFlight = null;
      session.problem = message.code === "STALE" ? "conflict" : message.code === "MISSING" ? "missing" : "error";
      session.message = message.message;
      settle(session);
      notify();
      return;
    }
    case "EditorFileChanged": {
      const session = sessions.get(message.sessionId);
      if (!session) return;
      const clean = !isDirty(session) && !session.inFlight;
      if (clean && message.state === "modified") {
        // A fully saved buffer follows the disk.
        const handler = externalHandlers.get(session.sessionId);
        if (handler) handler("reload");
        else {
          session.reloadOnMount = true;
          session.cached = null;
          session.readText = null;
        }
        return;
      }
      if (session.timer) {
        clearTimeout(session.timer);
        session.timer = null;
      }
      session.problem = message.state === "missing" ? "missing" : "conflict";
      session.message = message.state === "missing"
        ? "The file was deleted or renamed outside the editor."
        : "The file changed on disk.";
      settle(session);
      notify();
      return;
    }
    case "EditorFlushRequested":
      void flushAllEditors().then((failed) => {
        if (failed === 0) client?.sendIntent("Quit", undefined, { editorsFlushed: true });
        else quitBlockedHandler?.(failed);
      });
      return;
  }
}

let quitBlockedHandler: ((failed: number) => void) | null = null;

/** Wire the controller to the GUI connection. Returns a disposer. */
export function attachEditorSessions(wsClient: WsClient, onQuitBlocked: (failed: number) => void): () => void {
  client = wsClient;
  quitBlockedHandler = onQuitBlocked;
  const unsubscribe = wsClient.subscribeMessages(handleMessage);
  return () => {
    unsubscribe();
    if (client === wsClient) client = null;
    quitBlockedHandler = null;
  };
}

// ── Cursor line, for quick preview following the editor ─────────────────────

export interface EditorCursor {
  /** 1-based line in the file (BOM and CRLF do not change line numbers). */
  line: number;
  /** Where the cursor line sits in the editor viewport, 0 (top) … 1 (bottom). */
  fraction: number;
}

const cursors = new Map<string, EditorCursor>();
const cursorListeners = new Map<string, Set<(cursor: EditorCursor) => void>>();

export function publishEditorCursor(filePath: string, cursor: EditorCursor) {
  cursors.set(filePath, cursor);
  for (const listener of cursorListeners.get(filePath) ?? []) listener(cursor);
}

export function lastEditorCursor(filePath: string): EditorCursor | undefined {
  return cursors.get(filePath);
}

export function subscribeEditorCursor(filePath: string, listener: (cursor: EditorCursor) => void): () => void {
  let set = cursorListeners.get(filePath);
  if (!set) cursorListeners.set(filePath, (set = new Set()));
  set.add(listener);
  return () => {
    set.delete(listener);
    if (set.size === 0) cursorListeners.delete(filePath);
  };
}
