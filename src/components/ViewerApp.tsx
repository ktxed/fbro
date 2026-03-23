import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { WsClient } from "../ws/client";
import type { BEMessage, FileChunkMessage } from "../ws/types";

const SMALL_LIMIT = 32768;  // 32 KiB — files at or below this are loaded fully
const CHUNK_SIZE  = 65536;  // 64 KiB — buffer window for large files
const LINE_H      = 18;     // px — 12px font × 1.5 line-height

interface ViewerAppProps {
  path: string;
  sourceId: string;
  filename: string;
}

type ViewMode = "text" | "hex";

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

export function ViewerApp({ path, sourceId, filename }: ViewerAppProps) {
  const [mode, setMode]           = useState<ViewMode | null>(null);
  const [fileSize, setFileSize]   = useState<number | null>(null);
  const [loading, setLoading]     = useState(true);
  const [error, setError]         = useState<string | null>(null);

  // Buffer
  const [buffer, setBuffer]             = useState<Uint8Array | null>(null);
  const [bufferOffset, setBufferOffset] = useState(0);
  const [eof, setEof]                   = useState(false);

  // null = undetermined, true = small file, false = large file
  const [isSmall, setIsSmall] = useState<boolean | null>(null);

  // Large-file virtual scroll: line index at the top of viewport
  const [topLine, setTopLine] = useState(0);
  // Viewport height in lines
  const [vpLines, setVpLines] = useState(30);
  // Incremented each time a new buffer arrives — drives post-load effects
  const [bufSeq, setBufSeq]   = useState(0);

  const clientRef    = useRef<WsClient | null>(null);
  const bodyRef      = useRef<HTMLDivElement>(null);
  const contentRef   = useRef<HTMLPreElement>(null);
  // After a large-mode buffer load, what to do with topLine
  const afterLoadRef = useRef<"top" | "bottom">("top");

  const effectiveMode: ViewMode = mode ?? "text";

  // ── Derived: lines from buffer ────────────────────────────────────────────

  const lines = useMemo(() => {
    if (!buffer) return [];
    if (effectiveMode === "hex") return renderHexLines(buffer, bufferOffset);
    return renderText(buffer).split("\n");
  }, [buffer, bufferOffset, effectiveMode]);

  const maxTopLine = Math.max(0, lines.length - vpLines);

  // ── Request chunk from BE ─────────────────────────────────────────────────

  const requestChunk = useCallback((off: number, size: number, after: "top" | "bottom" = "top") => {
    afterLoadRef.current = after;
    setLoading(true);
    clientRef.current?.sendIntent("ReadFileChunk", undefined, {
      path, sourceId, offset: off, size,
    });
  }, [path, sourceId]);

  // ── Measure viewport on resize ────────────────────────────────────────────

  useEffect(() => {
    const measure = () => {
      const el = bodyRef.current;
      if (el) setVpLines(Math.max(1, Math.floor(el.clientHeight / LINE_H)));
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

    client.onConnected = () => requestChunk(0, SMALL_LIMIT);

    client.onMessage = (msg: BEMessage) => {
      if (msg.type === "FileChunk") {
        const m = msg as FileChunkMessage;
        const bytes = hexDecode(m.data);
        if (m.fileSize !== undefined) setFileSize(m.fileSize);
        setBuffer(bytes);
        setBufferOffset(m.offset);
        setEof(m.eof);
        setLoading(false);
        setMode(prev => prev ?? (detectBinary(bytes) ? "hex" : "text"));
        setIsSmall(prev => prev !== null ? prev : (m.eof && bytes.length <= SMALL_LIMIT));
        setBufSeq(s => s + 1);
      }
      if (msg.type === "Error") {
        setError((msg as any).message ?? "Unknown error");
        setLoading(false);
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
      if (e.key === "1") { setMode("text"); return; }
      if (e.key === "3") { setMode("hex"); return; }

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
  }, [isSmall, eof, loading, fileSize, bufferOffset, maxTopLine, vpLines, scrollByLines, requestChunk]);

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
    if (!buffer) return "";
    if (isSmall === true) {
      if (effectiveMode === "hex") return renderHexLines(buffer, 0).join("\n");
      return renderText(buffer);
    }
    // Large mode: visible slice
    const start = clamp(topLine, 0, lines.length);
    const end = Math.min(lines.length, topLine + vpLines + 1);
    return lines.slice(start, end).join("\n");
  }, [buffer, isSmall, effectiveMode, topLine, vpLines, lines]);

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

  return (
    <div className="viewer">
      <div className="viewer__header">
        <span className="viewer__filename">{filename}</span>
        <div className="viewer__mode-btns">
          <button
            className={`viewer__mode-btn${effectiveMode === "text" ? " viewer__mode-btn--active" : ""}`}
            onClick={() => setMode("text")}
            title="Text mode (1)"
          >
            1 Text
          </button>
          <button
            className={`viewer__mode-btn${effectiveMode === "hex" ? " viewer__mode-btn--active" : ""}`}
            onClick={() => setMode("hex")}
            title="Hex mode (3)"
          >
            3 Hex
          </button>
        </div>
      </div>

      <div className="viewer__body" ref={bodyRef}>
        {loading && !buffer && <div className="viewer__loading">Loading…</div>}
        {error && <div className="viewer__error">{error}</div>}
        {!error && buffer && (
          <>
            <pre
              className={`viewer__pre${isSmall === false ? " viewer__pre--virtual" : ""}`}
              ref={contentRef}
            >
              {content}
            </pre>
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
        <span className="viewer__status-nav">↑↓ PgDn/PgUp — navigate · 1 text · 3 hex</span>
      </div>
    </div>
  );
}
