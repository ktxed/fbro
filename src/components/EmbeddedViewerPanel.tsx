import React, { useEffect, useRef, useState, useCallback } from "react";
import { WsClient } from "../ws/client";
import type { AppState } from "../store";
import type { FileChunkMessage, ViewerPrefetchHintsMessage } from "../ws/types";
import { canUseDirect, DirectFileViewer } from "../viewers/DirectFileViewer";
import { ImageViewer, IMAGE_LIMIT } from "../viewers/ImageViewer";
import { isImageExt } from "../viewers/media";
import { hexDecode, detectBinary } from "../viewers/utils";

const SMALL_LIMIT = 1 * 1024 * 1024;   // 1 MiB — same as viewer
const DEBOUNCE_MS = 80;
const HEX_COLS = 16;

interface FileInfo {
  path: string;
  sourceId: string;
  filename: string;
  fileSize: number | null;
  isLocal: boolean;
}

type ViewMode =
  | { kind: "empty" }
  | { kind: "directory"; name: string }
  | { kind: "direct"; file: FileInfo }
  | { kind: "loading"; file: FileInfo }
  | { kind: "ready"; file: FileInfo; buffer: Uint8Array; mode: "text" | "image" | "hex" }
  | { kind: "too-large"; file: FileInfo }
  | { kind: "error"; file: FileInfo; message: string };

interface Props {
  sourcePanelId: string;
  state: AppState;
}

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

function HexLines({ bytes }: { bytes: Uint8Array }) {
  const lines: React.ReactElement[] = [];
  const maxRows = 64; // show at most 64 rows = 1024 bytes
  const limit = Math.min(bytes.length, maxRows * HEX_COLS);
  for (let off = 0; off < limit; off += HEX_COLS) {
    const row = bytes.slice(off, off + HEX_COLS);
    const hex = Array.from(row).map(b => b.toString(16).padStart(2, "0")).join(" ");
    const asc = Array.from(row).map(b => (b >= 0x20 && b < 0x7f) ? String.fromCharCode(b) : ".").join("");
    lines.push(
      <div key={off} className="embedded-viewer__hex-row">
        <span className="embedded-viewer__hex-addr">{off.toString(16).padStart(8, "0")}</span>
        <span className="embedded-viewer__hex-bytes">{hex.padEnd(HEX_COLS * 3 - 1)}</span>
        <span className="embedded-viewer__hex-asc">{asc}</span>
      </div>
    );
  }
  if (bytes.length > limit) {
    lines.push(
      <div key="ellipsis" className="embedded-viewer__hex-ellipsis">
        … {bytes.length - limit} more bytes
      </div>
    );
  }
  return <>{lines}</>;
}

export function EmbeddedViewerPanel({ sourcePanelId, state }: Props) {
  const [viewMode, setViewMode] = useState<ViewMode>({ kind: "empty" });
  const clientRef = useRef<WsClient | null>(null);
  const pendingPathRef = useRef<string | null>(null);
  const fsPortRef = useRef(state.fileServerPort);
  fsPortRef.current = state.fileServerPort;
  // Tracks the path we have already loaded/are loading — prevents re-requesting
  // the same file when the panel receives unrelated state patches (which increment
  // `version`) while the cursor hasn't actually moved.
  const loadedPathRef = useRef<string | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Create own WsClient for ReadFileChunk isolation.
  useEffect(() => {
    const client = new WsClient("viewer");
    clientRef.current = client;

    client.onMessage = (msg) => {
      if (msg.type === "FileChunk") {
        const m = msg as FileChunkMessage;
        // Ignore stale chunks (cursor moved while chunk was in flight).
        if (m.path !== pendingPathRef.current) return;
        pendingPathRef.current = null;

        const bytes = hexDecode(m.data);
        setViewMode(prev => {
          if (prev.kind !== "loading") return prev;
          const { file } = prev;
          const isBin = detectBinary(bytes);
          const rendMode = isImageExt(file.filename)
            ? (file.fileSize !== null && file.fileSize > IMAGE_LIMIT ? "hex" : "image")
            : isBin ? "hex" : "text";
          return { kind: "ready", file, buffer: bytes, mode: rendMode };
        });

        // Schedule BE prefetch for neighboring files and receive FE preload hints.
        client.sendIntent("PrefetchViewerNeighbors", undefined, { panelId: sourcePanelId });
        return;
      }

      if (msg.type === "ViewerPrefetchHints") {
        const m = msg as ViewerPrefetchHintsMessage;
        for (const hint of m.hints) {
          if (canUseDirect(hint.isLocal, hint.filename, hint.fileSize ?? null)) {
            const img = new Image();
            img.src = `http://127.0.0.1:${fsPortRef.current}${hint.path}`;
          }
        }
      }
    };

    client.connect();
    return () => {
      client.destroy();
      clientRef.current = null;
    };
  }, []);

  const loadFile = useCallback((info: FileInfo) => {
    pendingPathRef.current = info.path;
    const size = isImageExt(info.filename) ? IMAGE_LIMIT : SMALL_LIMIT;
    clientRef.current?.sendIntent("ReadFileChunk", undefined, {
      path: info.path,
      sourceId: info.sourceId,
      offset: 0,
      size,
    });
    setViewMode({ kind: "loading", file: info });
  }, []);

  // Reset loaded-path guard when the directory changes so navigating into a
  // new folder and back shows fresh content.
  const panelPath = state.panels[sourcePanelId]?.path;
  useEffect(() => {
    loadedPathRef.current = null;
  }, [panelPath, sourcePanelId]);

  // Watch source panel cursor. Debounce to avoid flooding on fast scroll.
  useEffect(() => {
    const panel = state.panels[sourcePanelId];
    if (!panel) return;

    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      // In windowed (virtual scroll) mode panel.entries is only the loaded
      // window slice; panel.windowStart holds the absolute offset of that
      // window. Use the window-relative index to find the current entry.
      const windowStart = panel.windowStart ?? 0;
      const localIndex = panel.cursor - windowStart;
      const entry = localIndex >= 0 && localIndex < panel.entries.length
        ? panel.entries[localIndex]
        : undefined;

      if (!entry) {
        // Cursor is outside the currently loaded window (large directory,
        // fast scroll). Keep the existing preview — the window will catch up
        // shortly and version will increment, re-triggering this effect.
        if ((panel.totalCount ?? panel.entries.length) > 0) return;
        // Genuinely empty directory.
        loadedPathRef.current = null;
        setViewMode({ kind: "empty" });
        return;
      }
      if (entry.kind === "directory") {
        loadedPathRef.current = null;
        setViewMode({ kind: "directory", name: entry.name });
        return;
      }

      const source = state.sources[panel.sourceId];
      const isLocal = source?.kind === "filesystem";
      const base = panel.path.endsWith("/") ? panel.path : panel.path + "/";
      const path = panel.path ? `${base}${entry.name}` : entry.name;

      // Guard: skip if we're already showing or loading this exact file.
      // This prevents duplicate ReadFileChunk requests when unrelated panel
      // patches increment `version` while the cursor hasn't moved.
      if (path === loadedPathRef.current) return;
      loadedPathRef.current = path;

      const fileSize = entry.size ?? null;
      const info: FileInfo = {
        path,
        sourceId: panel.sourceId,
        filename: entry.name,
        fileSize,
        isLocal,
      };

      // For local images/PDF/video: serve directly, no round trip needed.
      if (canUseDirect(isLocal, entry.name, fileSize)) {
        pendingPathRef.current = null;
        setViewMode({ kind: "direct", file: info });
        clientRef.current?.sendIntent("PrefetchViewerNeighbors", undefined, { panelId: sourcePanelId });
        return;
      }

      // Skip files that are too large to preview.
      if (fileSize !== null && fileSize > IMAGE_LIMIT) {
        setViewMode({ kind: "too-large", file: info });
        return;
      }

      loadFile(info);
    }, DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [
    state.panels[sourcePanelId]?.cursor,
    state.panels[sourcePanelId]?.sourceId,
    state.panels[sourcePanelId]?.path,
    state.panels[sourcePanelId]?.version,
    state.sources,
    sourcePanelId,
    loadFile,
  ]);

  // Header label
  const headerLabel = (() => {
    if (viewMode.kind === "directory") return `📁 ${viewMode.name}`;
    if (viewMode.kind === "empty") return "No file selected";
    const { file } = viewMode as { file: FileInfo };
    return `${file.filename}${file.fileSize !== null ? "  ·  " + formatSize(file.fileSize) : ""}`;
  })();

  return (
    <div className="embedded-viewer" onMouseDown={(e) => e.preventDefault()}>
      <div className="embedded-viewer__header">{headerLabel}</div>
      <div className="embedded-viewer__content" style={{ height: "100%" }}>
        {viewMode.kind === "empty" && (
          <div className="embedded-viewer__placeholder">No file selected</div>
        )}
        {viewMode.kind === "directory" && (
          <div className="embedded-viewer__placeholder">📁 Directory</div>
        )}
        {viewMode.kind === "loading" && (
          <div className="embedded-viewer__placeholder">Loading…</div>
        )}
        {viewMode.kind === "too-large" && (
          <div className="embedded-viewer__placeholder">
            File too large to preview
            {viewMode.file.fileSize !== null && ` (${formatSize(viewMode.file.fileSize)})`}
          </div>
        )}
        {viewMode.kind === "error" && (
          <div className="embedded-viewer__placeholder">Error: {viewMode.message}</div>
        )}
        {viewMode.kind === "direct" && (
          <DirectFileViewer
            path={viewMode.file.path}
            filename={viewMode.file.filename}
            fsPort={state.fileServerPort}
          />
        )}
        {viewMode.kind === "ready" && viewMode.mode === "image" && (
          <ImageViewer
            buffer={viewMode.buffer}
            fileSize={viewMode.file.fileSize}
            filename={viewMode.file.filename}
          />
        )}
        {viewMode.kind === "ready" && viewMode.mode === "text" && (
          <pre className="embedded-viewer__text">
            {new TextDecoder("utf-8", { fatal: false }).decode(viewMode.buffer)}
          </pre>
        )}
        {viewMode.kind === "ready" && viewMode.mode === "hex" && (
          <div className="embedded-viewer__hex">
            <HexLines bytes={viewMode.buffer} />
          </div>
        )}
      </div>
    </div>
  );
}
