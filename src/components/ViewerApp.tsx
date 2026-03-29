import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { WsClient } from "../ws/client";
import type { BEMessage, FileChunkMessage, ViewerFileChangedMessage } from "../ws/types";
import { ImageViewer, IMAGE_LIMIT } from "../viewers/ImageViewer";
import { detectRenderer, isImageExt } from "../viewers/media";
import { DirectFileViewer, canUseDirect } from "../viewers/DirectFileViewer";

const SMALL_LIMIT = 32768;  // 32 KiB — files at or below this are loaded fully
const CHUNK_SIZE  = 65536;  // 64 KiB — buffer window for large files
const LINE_H      = 20;     // px — 13px font × 1.5 line-height (19.5, rounded up)
const MAX_COL     = 512;    // chars — wrap long lines into multiple display rows

interface ViewerAppProps {
  path: string;
  sourceId: string;
  filename: string;
  windowLabel: string;
  panelId: string;
  isLocal: boolean;
  nativeFileSize: number | null;
}

type ViewMode = "text" | "image" | "hex";

// ─── Helpers ─────────────────────────────────────────────────────────────────

function hexDecode(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

function detectBinary(bytes: Uint8Array): boolean {
  const sample = bytes.slice(0, 4096);
  let nonText = 0;
  for (const b of sample) {
    if (b === 0) return true;
    if ((b < 9) || (b > 13 && b < 32) || b === 127) nonText++;
  }
  return sample.length > 0 && nonText / sample.length > 0.3;
}

function renderText(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

function renderHexLines(bytes: Uint8Array, baseOffset: number): string[] {
  const result: string[] = [];
  for (let i = 0; i < bytes.length; i += 16) {
    const row = bytes.slice(i, i + 16);
    const addr = (baseOffset + i).toString(16).padStart(8, "0");
    const hex = Array.from({ length: 16 }, (_, j) => {
      if (j === 8) return " " + (j < row.length ? row[j].toString(16).padStart(2, "0") : "  ");
      return j < row.length ? row[j].toString(16).padStart(2, "0") : "  ";
    }).join(" ");
    const ascii = Array.from(row)
      .map(b => (b >= 32 && b < 127) ? String.fromCharCode(b) : ".")
      .join("");
    result.push(`${addr}  ${hex}  |${ascii}|`);
  }
  return result;
}

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}

// ─── Component ────────────────────────────────────────────────────────────────

export function ViewerApp({ path: initPath, sourceId: initSourceId, filename: initFilename, windowLabel, panelId, isLocal, nativeFileSize }: ViewerAppProps) {
  // Current file being viewed (updated on navigation)
  const [curPath, setCurPath]         = useState(initPath);
  const [curSourceId, setCurSourceId] = useState(initSourceId);
  const [curFilename, setCurFilename] = useState(initFilename);

  const [mode, setMode]           = useState<ViewMode | null>(null);
  const [fileSize, setFileSize]   = useState<number | null>(null);
  const [loading, setLoading]     = useState(true);
  const [error, setError]         = useState<string | null>(null);
  const [navError, setNavError]   = useState<string | null>(null);

  // Buffer
  const [buffer, setBuffer]             = useState<Uint8Array | null>(null);
  const [bufferOffset, setBufferOffset] = useState(0);
  const [eof, setEof]                   = useState(false);

  // null = undetermined, true = small file, false = large file
  const [isSmall, setIsSmall] = useState<boolean | null>(null);

  // true = use file:// direct rendering (local images < 10 MiB, video, PDF)
  const [useDirect, setUseDirect] = useState(() => canUseDirect(isLocal, initFilename, nativeFileSize));
  // true = current file supports direct rendering (even if user switched to text/hex)
  const [canDirect, setCanDirect] = useState(() => canUseDirect(isLocal, initFilename, nativeFileSize));

  // Large-file virtual scroll: line index at the top of viewport
  const [topLine, setTopLine] = useState(0);
  // Viewport height in lines
  const [vpLines, setVpLines] = useState(30);
  // Incremented each time a new buffer arrives — drives post-load effects
  const [bufSeq, setBufSeq]   = useState(0);

  const clientRef     = useRef<WsClient | null>(null);
  const bodyRef       = useRef<HTMLDivElement>(null);
  const contentRef    = useRef<HTMLElement>(null);
  // After a large-mode buffer load, what to do with topLine
  const afterLoadRef  = useRef<"top" | "bottom">("top");
  // Refs so requestChunk can always use the current path/sourceId without
  // being recreated on navigation (which would tear down and recreate the WS).
  const curPathRef      = useRef(initPath);
  const curSourceIdRef  = useRef(initSourceId);
  const filenameRef   = useRef(initFilename);

  // 1-based file line number of the first raw line in the current buffer.
  // null = unknown (after a scrollbar jump to an arbitrary byte offset).
  const [bufferBaseLine, setBufferBaseLine] = useState<number | null>(1);
  // Tracks state needed to compute baseLine when the next chunk arrives.
  const chunkInfoRef = useRef<{ end: number; newlineCount: number; baseLine: number | null }>({
    end: 0, newlineCount: 0, baseLine: 1,
  });

  filenameRef.current  = curFilename;
  curPathRef.current     = curPath;
  curSourceIdRef.current = curSourceId;
  const effectiveMode: ViewMode = useDirect ? "image" : (mode ?? "text");
  const isImageMode = effectiveMode === "image";

  function switchMode(newMode: ViewMode) {
    if (newMode === "image" && canDirect) {
      setUseDirect(true);
      setMode("image");
      return;
    }
    if (useDirect) {
      setUseDirect(false);
    }
    setMode(newMode);
    if (!buffer) {
      requestChunk(0, SMALL_LIMIT);
    }
  }

  // ── Derived: lines from buffer ────────────────────────────────────────────

  // lines: display rows (long file lines are wrapped into multiple rows)
  // lineNums: 1-based file line number for each display row (text mode only);
  //           null entry means position unknown (chunk loaded at arbitrary offset)
  const { lines, lineNums } = useMemo(() => {
    if (!buffer) return { lines: [] as string[], lineNums: [] as (number | null)[] };
    if (effectiveMode === "hex") {
      return { lines: renderHexLines(buffer, bufferOffset), lineNums: [] as (number | null)[] };
    }
    const raw = renderText(buffer).split("\n");
    const lines: string[] = [];
    const lineNums: (number | null)[] = [];
    // bufferBaseLine = null means we jumped to an unknown position
    const base = bufferBaseLine;
    for (let i = 0; i < raw.length; i++) {
      const line = raw[i];
      const fileLineNum = base !== null ? base + i : null;
      if (line.length <= MAX_COL) {
        lines.push(line);
        lineNums.push(fileLineNum);
      } else {
        for (let j = 0; j < line.length; j += MAX_COL) {
          lines.push(line.slice(j, j + MAX_COL));
          lineNums.push(fileLineNum);
        }
      }
    }
    return { lines, lineNums };
  }, [buffer, bufferOffset, effectiveMode, bufferBaseLine]);

  const maxTopLine = Math.max(0, lines.length - vpLines);

  // ── Request chunk from BE ─────────────────────────────────────────────────

  const requestChunk = useCallback((off: number, size: number, after: "top" | "bottom" = "top") => {
    afterLoadRef.current = after;
    setLoading(true);
    clientRef.current?.sendIntent("ReadFileChunk", undefined, {
      path: curPathRef.current, sourceId: curSourceIdRef.current, offset: off, size,
    });
  }, []); // stable: reads current path/sourceId via refs, never recreated

  // ── Measure viewport on resize ────────────────────────────────────────────

  useEffect(() => {
    const measure = () => {
      const el = bodyRef.current;
      // Reserve one LINE_H at the bottom: accounts for the 8px top padding
      // and leaves a visible gap so the last line isn't flush with the edge.
      if (el) setVpLines(Math.max(1, Math.floor((el.clientHeight - LINE_H) / LINE_H)));
    };
    measure();
    const obs = new ResizeObserver(measure);
    if (bodyRef.current) obs.observe(bodyRef.current);
    return () => obs.disconnect();
  }, []);

  // ── WS connection ─────────────────────────────────────────────────────────

  useEffect(() => {
    const client = new WsClient("viewer");
    clientRef.current = client;

    client.onConnected = () => {
      if (canUseDirect(isLocal, initFilename, nativeFileSize)) {
        setLoading(false);
      } else {
        // For known image types, request up to IMAGE_LIMIT in one shot
        const initSize = isImageExt(initFilename) ? IMAGE_LIMIT : SMALL_LIMIT;
        requestChunk(0, initSize);
      }
    };

    client.onMessage = (msg: BEMessage) => {
      if (msg.type === "FileChunk") {
        const m = msg as FileChunkMessage;
        const bytes = hexDecode(m.data);

        // Compute the 1-based file line number of the first raw line in this chunk.
        // Use chunkInfoRef (a ref, so always fresh) to compare with previous chunk.
        const info = chunkInfoRef.current;
        let newBaseLine: number | null;
        if (m.offset === 0) {
          newBaseLine = 1;
        } else if (m.offset === info.end) {
          // Sequential load: base advances by the newline count of the previous buffer.
          newBaseLine = info.baseLine !== null ? info.baseLine + info.newlineCount : null;
        } else {
          // Jump to arbitrary byte position — absolute line number is unknown.
          newBaseLine = null;
        }
        let newlineCount = 0;
        for (const b of bytes) if (b === 10) newlineCount++;
        chunkInfoRef.current = { end: m.offset + bytes.length, newlineCount, baseLine: newBaseLine };
        setBufferBaseLine(newBaseLine);

        if (m.fileSize !== undefined) setFileSize(m.fileSize);
        setBuffer(bytes);
        setBufferOffset(m.offset);
        setEof(m.eof);
        setLoading(false);
        setMode(prev => {
          if (prev !== null) return prev;
          const detected = detectRenderer(filenameRef.current, detectBinary(bytes));
          if (detected === "image" && m.fileSize !== undefined && m.fileSize > IMAGE_LIMIT) return "hex";
          return detected;
        });
        setNavError(null);
        setIsSmall(prev => prev !== null ? prev : (m.eof && bytes.length <= SMALL_LIMIT));
        setBufSeq(s => s + 1);
      }
      if (msg.type === "ViewerFileChanged") {
        const m = msg as ViewerFileChangedMessage;
        const direct = canUseDirect(m.isLocal ?? false, m.filename, m.fileSize ?? null);
        // Reset viewer state for new file
        setCurPath(m.path);
        setCurSourceId(m.sourceId);
        setCurFilename(m.filename);
        setUseDirect(direct);
        setCanDirect(direct);
        setMode(null);
        setBuffer(null);
        setBufferOffset(0);
        setEof(false);
        setIsSmall(null);
        setTopLine(0);
        setFileSize(null);
        setError(null);
        setNavError(null);
        setLoading(!direct);
        setBufferBaseLine(1);
        chunkInfoRef.current = { end: 0, newlineCount: 0, baseLine: 1 };
        setBufSeq(0);
        if (!direct) {
          // Request first chunk of new file
          const initSize = isImageExt(m.filename) ? IMAGE_LIMIT : SMALL_LIMIT;
          clientRef.current?.sendIntent("ReadFileChunk", undefined, {
            path: m.path, sourceId: m.sourceId, offset: 0, size: initSize,
          });
        }
      }
      if (msg.type === "Error") {
        const err = msg as { code: string; message: string };
        if (err.code === "NOT_A_FILE") {
          setNavError(err.message);
        } else {
          setError(err.message ?? "Unknown error");
          setLoading(false);
        }
      }
    };

    client.connect();
    return () => client.destroy();
  }, [requestChunk]);

  // ── After large-mode buffer loads, set topLine ────────────────────────────

  useEffect(() => {
    if (bufSeq === 0 || isSmall !== false) return;
    if (afterLoadRef.current === "bottom") {
      setTopLine(Math.max(0, lines.length - vpLines));
    } else {
      setTopLine(0);
    }
  }, [bufSeq]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Clamp topLine when mode changes (hex ↔ text changes line count) ──────

  useEffect(() => {
    if (isSmall !== false) return;
    setTopLine(prev => clamp(prev, 0, maxTopLine));
  }, [effectiveMode, maxTopLine, isSmall]);

  // ── Large-mode navigation helpers ─────────────────────────────────────────

  const scrollByLines = useCallback((delta: number) => {
    setTopLine(prev => {
      const next = clamp(prev + delta, 0, maxTopLine);
      // Hit bottom edge of buffer — need next chunk?
      if (next >= maxTopLine && delta > 0 && !eof && !loading && buffer) {
        const nextOff = bufferOffset + buffer.length;
        if (fileSize === null || nextOff < fileSize) {
          requestChunk(nextOff, CHUNK_SIZE, "top");
          return prev; // keep current until new buffer arrives
        }
      }
      // Hit top edge — need previous chunk?
      if (next <= 0 && delta < 0 && bufferOffset > 0 && !loading) {
        const prevOff = Math.max(0, bufferOffset - CHUNK_SIZE);
        requestChunk(prevOff, CHUNK_SIZE, "bottom");
        return prev;
      }
      return next;
    });
  }, [maxTopLine, eof, loading, buffer, bufferOffset, fileSize, requestChunk]);

  // ── Keyboard ──────────────────────────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        clientRef.current?.sendIntent("CloseViewerWindow", undefined, { label: windowLabel });
        return;
      }
      if (e.key === "1") { switchMode("text"); return; }
      if (e.key === "2") { switchMode("image"); return; }
      if (e.key === "3") { switchMode("hex"); return; }
      if (e.key === "k") {
        e.preventDefault();
        setNavError(null);
        clientRef.current?.sendIntent("ViewerNavigate", undefined, { panelId, direction: "next" });
        return;
      }
      if (e.key === "j") {
        e.preventDefault();
        setNavError(null);
        clientRef.current?.sendIntent("ViewerNavigate", undefined, { panelId, direction: "prev" });
        return;
      }

      const goStart = e.key === "Home" || (e.key === "ArrowUp" && e.metaKey);
      const goEnd   = e.key === "End"  || (e.key === "ArrowDown" && e.metaKey);

      // ── Small file mode: native DOM scroll ────────────────────────────
      if (isSmall === true) {
        const el = contentRef.current;
        if (!el) return;
        if (goStart) { e.preventDefault(); el.scrollTop = 0; return; }
        if (goEnd)   { e.preventDefault(); el.scrollTop = el.scrollHeight; return; }
        if (e.key === "ArrowDown")  { e.preventDefault(); el.scrollBy({ top: LINE_H }); return; }
        if (e.key === "ArrowUp")    { e.preventDefault(); el.scrollBy({ top: -LINE_H }); return; }
        if (e.key === "PageDown" || (e.key === " " && !e.shiftKey)) {
          e.preventDefault(); el.scrollBy({ top: el.clientHeight }); return;
        }
        if (e.key === "PageUp" || (e.key === " " && e.shiftKey)) {
          e.preventDefault(); el.scrollBy({ top: -el.clientHeight }); return;
        }
        return;
      }

      // ── Large file mode: virtual scroll ───────────────────────────────
      if (isSmall === false) {
        if (goStart) {
          e.preventDefault();
          if (bufferOffset === 0) { setTopLine(0); }
          else if (!loading) { requestChunk(0, CHUNK_SIZE, "top"); }
          return;
        }
        if (goEnd) {
          e.preventDefault();
          if (eof) { setTopLine(maxTopLine); }
          else if (!loading && fileSize !== null) {
            const lastOff = Math.max(0, fileSize - CHUNK_SIZE);
            requestChunk(lastOff, CHUNK_SIZE, "bottom");
          }
          return;
        }
        if (e.key === "ArrowDown")  { e.preventDefault(); scrollByLines(1); return; }
        if (e.key === "ArrowUp")    { e.preventDefault(); scrollByLines(-1); return; }
        if (e.key === "PageDown" || (e.key === " " && !e.shiftKey)) {
          e.preventDefault(); scrollByLines(vpLines); return;
        }
        if (e.key === "PageUp" || (e.key === " " && e.shiftKey)) {
          e.preventDefault(); scrollByLines(-vpLines); return;
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [isSmall, eof, loading, fileSize, bufferOffset, maxTopLine, vpLines, scrollByLines, requestChunk, windowLabel]);

  // ── Mouse wheel for large mode ────────────────────────────────────────────

  useEffect(() => {
    if (isSmall !== false) return;
    const el = bodyRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => {
      e.preventDefault();
      const deltaLines = Math.round(e.deltaY / LINE_H) || (e.deltaY > 0 ? 1 : -1);
      scrollByLines(deltaLines);
    };
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  }, [isSmall, scrollByLines]);

  // ── Scrollbar drag for large mode ─────────────────────────────────────────

  const handleScrollbarMouse = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    if (fileSize === null || fileSize === 0 || loading) return;
    const track = e.currentTarget;
    const rect = track.getBoundingClientRect();

    const jump = (clientY: number) => {
      const ratio = clamp((clientY - rect.top) / rect.height, 0, 1);
      const targetByte = Math.floor(ratio * fileSize);
      // If target is within current buffer, just adjust topLine
      if (buffer && targetByte >= bufferOffset && targetByte < bufferOffset + buffer.length) {
        const byteFrac = (targetByte - bufferOffset) / buffer.length;
        setTopLine(clamp(Math.floor(byteFrac * lines.length), 0, maxTopLine));
      } else {
        const loadOff = clamp(Math.floor(targetByte - CHUNK_SIZE / 2), 0, Math.max(0, fileSize - CHUNK_SIZE));
        requestChunk(loadOff, CHUNK_SIZE, "top");
        // After load, estimate topLine for the target byte within the chunk
        const fracInChunk = (targetByte - loadOff) / CHUNK_SIZE;
        afterLoadRef.current = "top"; // will be overridden below
        // We'll set topLine in the bufSeq effect — but we need a custom value.
        // Use a small hack: set a pending value via ref
        pendingJumpRef.current = fracInChunk;
      }
    };

    jump(e.clientY);

    const onMove = (ev: MouseEvent) => jump(ev.clientY);
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, [fileSize, loading, buffer, bufferOffset, lines.length, maxTopLine, requestChunk]);

  const pendingJumpRef = useRef<number | null>(null);

  // Handle pending jump fraction after buffer loads
  useEffect(() => {
    if (bufSeq === 0 || isSmall !== false || pendingJumpRef.current === null) return;
    const frac = pendingJumpRef.current;
    pendingJumpRef.current = null;
    setTopLine(clamp(Math.floor(frac * lines.length), 0, Math.max(0, lines.length - vpLines)));
  }, [bufSeq]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Scrollbar metrics ─────────────────────────────────────────────────────

  const scrollMetrics = useMemo(() => {
    if (isSmall !== false || fileSize === null || fileSize === 0 || !buffer) {
      return { thumbTop: 0, thumbHeight: 100 };
    }
    // Estimate current byte position at viewport top
    let bytePos: number;
    if (effectiveMode === "hex") {
      bytePos = bufferOffset + topLine * 16;
    } else {
      const frac = lines.length > 0 ? topLine / lines.length : 0;
      bytePos = bufferOffset + frac * buffer.length;
    }
    // Estimate viewport byte span
    let vpBytes: number;
    if (effectiveMode === "hex") {
      vpBytes = vpLines * 16;
    } else {
      const frac = lines.length > 0 ? vpLines / lines.length : 1;
      vpBytes = frac * buffer.length;
    }
    const thumbTop = (bytePos / fileSize) * 100;
    const thumbHeight = clamp((vpBytes / fileSize) * 100, 1.5, 100);
    return { thumbTop: clamp(thumbTop, 0, 100 - thumbHeight), thumbHeight };
  }, [isSmall, fileSize, buffer, bufferOffset, topLine, vpLines, lines.length, effectiveMode]);

  // ── Render content ────────────────────────────────────────────────────────

  const content = useMemo(() => {
    if (!buffer || effectiveMode !== "hex") return "";
    if (isSmall === true) return renderHexLines(buffer, 0).join("\n");
    // Large mode: visible slice
    const start = clamp(topLine, 0, lines.length);
    const end = Math.min(lines.length, topLine + vpLines + 1);
    return lines.slice(start, end).join("\n");
  }, [buffer, isSmall, effectiveMode, topLine, vpLines, lines]);

  // Text mode: line-numbered view
  const textView = useMemo(() => {
    if (effectiveMode !== "text" || !buffer) return { gutterText: "", textContent: "" };
    let visLines: string[];
    let visNums: (number | null)[];
    if (isSmall === true) {
      visLines = lines;
      visNums = lineNums;
    } else {
      const start = clamp(topLine, 0, lines.length);
      const end = Math.min(lines.length, start + vpLines + 1);
      visLines = lines.slice(start, end);
      visNums = lineNums.slice(start, end);
    }
    const lastKnown = [...visNums].reverse().find(n => n !== null) ?? 1;
    const w = Math.max(4, (lastKnown as number).toString().length);
    // Show file line number only on the first display row of each file line
    const gutterEntries = visNums.map((num, i) => {
      const isFirst = i === 0 || visNums[i - 1] !== num;
      if (!isFirst) return " ".repeat(w);
      if (num === null) return "~".repeat(w); // unknown position after a jump
      return num.toString().padStart(w);
    });
    return { gutterText: gutterEntries.join("\n"), textContent: visLines.join("\n") };
  }, [effectiveMode, buffer, isSmall, lines, lineNums, topLine, vpLines]);

  // ── Status bar info ───────────────────────────────────────────────────────

  let offsetDisplay = bufferOffset;
  if (isSmall === false && effectiveMode === "hex") {
    offsetDisplay = bufferOffset + topLine * 16;
  } else if (isSmall === false && lines.length > 0 && buffer) {
    offsetDisplay = bufferOffset + Math.round((topLine / lines.length) * buffer.length);
  }

  const pct = fileSize && fileSize > 0
    ? Math.min(100, Math.round((offsetDisplay / fileSize) * 100))
    : isSmall ? 100 : 0;

  if (useDirect) {
    return (
      <div className="viewer">
        <div className="viewer__header">
          <span className="viewer__filename">{curFilename}</span>
          <div className="viewer__mode-btns">
            <button
              className={`viewer__mode-btn${effectiveMode === "text" ? " viewer__mode-btn--active" : ""}`}
              onClick={() => switchMode("text")}
              title="Text mode (1)"
            >
              1 Text
            </button>
            <button
              className={`viewer__mode-btn${effectiveMode === "image" ? " viewer__mode-btn--active" : ""}`}
              onClick={() => switchMode("image")}
              title="Image mode (2)"
            >
              2 Image
            </button>
            <button
              className={`viewer__mode-btn${effectiveMode === "hex" ? " viewer__mode-btn--active" : ""}`}
              onClick={() => switchMode("hex")}
              title="Hex mode (3)"
            >
              3 Hex
            </button>
          </div>
        </div>
        <div className="viewer__body" ref={bodyRef}>
          {navError ? (
            <div className="viewer__nav-error">
              <p>{navError}</p>
              <p className="viewer__nav-error-hint">
                j/k — try again · Esc — close viewer
              </p>
            </div>
          ) : (
            <DirectFileViewer path={curPath} filename={curFilename} />
          )}
        </div>
        <div className="viewer__status">
          <span className="viewer__status-nav">j/k prev/next file · Esc close</span>
        </div>
      </div>
    );
  }

  return (
    <div className="viewer">
      <div className="viewer__header">
        <span className="viewer__filename">{curFilename}</span>
        <div className="viewer__mode-btns">
          <button
            className={`viewer__mode-btn${effectiveMode === "text" ? " viewer__mode-btn--active" : ""}`}
            onClick={() => switchMode("text")}
            title="Text mode (1)"
          >
            1 Text
          </button>
          <button
            className={`viewer__mode-btn${effectiveMode === "image" ? " viewer__mode-btn--active" : ""}`}
            onClick={() => switchMode("image")}
            title="Image mode (2)"
          >
            2 Image
          </button>
          <button
            className={`viewer__mode-btn${effectiveMode === "hex" ? " viewer__mode-btn--active" : ""}`}
            onClick={() => switchMode("hex")}
            title="Hex mode (3)"
          >
            3 Hex
          </button>
        </div>
      </div>

      <div className="viewer__body" ref={bodyRef}>
        {loading && !buffer && <div className="viewer__loading">Loading…</div>}
        {error && <div className="viewer__error">{error}</div>}
        {navError && (
          <div className="viewer__nav-error">
            <p>{navError}</p>
            <p className="viewer__nav-error-hint">
              j/k — try again · Esc — close viewer
            </p>
          </div>
        )}
        {!error && buffer && isImageMode && (
          <ImageViewer buffer={buffer} fileSize={fileSize} filename={curFilename} />
        )}
        {!error && buffer && !isImageMode && (
          <>
            {effectiveMode === "hex" ? (
              <pre
                className={`viewer__pre${isSmall === false ? " viewer__pre--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLPreElement>}
              >
                {content}
              </pre>
            ) : (
              <div
                className={`viewer__text${isSmall === false ? " viewer__text--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLDivElement>}
              >
                <pre className="viewer__gutter" aria-hidden="true">
                  {textView.gutterText}
                </pre>
                <pre className="viewer__content">
                  {textView.textContent}
                </pre>
              </div>
            )}
            {isSmall === false && fileSize !== null && fileSize > 0 && (
              <div className="viewer__scrollbar" onMouseDown={handleScrollbarMouse}>
                <div
                  className="viewer__scrollbar-thumb"
                  style={{
                    top: `${scrollMetrics.thumbTop}%`,
                    height: `${scrollMetrics.thumbHeight}%`,
                  }}
                />
              </div>
            )}
          </>
        )}
      </div>

      <div className="viewer__status">
        <span>Offset: 0x{offsetDisplay.toString(16).padStart(8, "0")}</span>
        {fileSize !== null && <span>Size: {formatSize(fileSize)}</span>}
        <span>{pct}%</span>
        {loading && <span style={{ color: "var(--accent)" }}>loading…</span>}
        <span className="viewer__status-nav">
          {isImageMode
            ? "z zoom · j/k prev/next file · 1 text · 2 image · 3 hex"
            : "↑↓ PgDn/PgUp scroll · j/k prev/next file · 1 text · 2 image · 3 hex"}
        </span>
      </div>
    </div>
  );
}
