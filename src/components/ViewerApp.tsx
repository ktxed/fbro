import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { WsClient } from "../ws/client";
import type { BEMessage, FileChunkMessage, ViewerFileChangedMessage } from "../ws/types";
import { ImageViewer, IMAGE_LIMIT } from "../viewers/ImageViewer";
import { detectRenderer, isImageExt } from "../viewers/media";
import { DirectFileViewer, canUseDirect } from "../viewers/DirectFileViewer";

const SMALL_LIMIT = 1 * 1024 * 1024;  // 1 MiB — files at or below this are loaded fully
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

// ─── CP437 (DOS) rendering ────────────────────────────────────────────────────

// Mapping for bytes 0x80-0xFF in IBM Code Page 437 → Unicode
const CP437_HIGH =
  "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»" +
  "░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀" +
  "αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u00A0";

function renderCp437(bytes: Uint8Array): string {
  const chars: string[] = [];
  for (const b of bytes) {
    if (b >= 0x80) {
      chars.push(CP437_HIGH[b - 0x80]);
    } else if (b === 0x0D) {
      // skip CR (pairs with LF)
    } else if (b < 0x20 || b === 0x7F) {
      if (b === 0x0A) chars.push("\n");
      else chars.push(" "); // collapse other control chars
    } else {
      chars.push(String.fromCharCode(b));
    }
  }
  return chars.join("");
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

function highlightLine(
  text: string,
  matches: { line: number; start: number; end: number }[],
  lineIdx: number,
  activeGlobalIdx: number,
  firstGlobalIdxForLine: number,
): React.ReactNode {
  const lineMatches = matches.filter(m => m.line === lineIdx);
  if (lineMatches.length === 0) return text || "\u00A0";
  const parts: React.ReactNode[] = [];
  let pos = 0;
  for (let i = 0; i < lineMatches.length; i++) {
    const m = lineMatches[i];
    if (m.start > pos) parts.push(text.slice(pos, m.start));
    const isActive = firstGlobalIdxForLine + i === activeGlobalIdx;
    parts.push(
      <mark key={m.start} className={`viewer__find-match${isActive ? " viewer__find-match--active" : ""}`}>
        {text.slice(m.start, m.end)}
      </mark>
    );
    pos = m.end;
  }
  if (pos < text.length) parts.push(text.slice(pos));
  return parts;
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
  // eof is tracked via bufRangeRef (synchronous) rather than React state.

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
  // Synchronous loading guard — prevents duplicate chunk requests within the
  // same render cycle (React state `loading` only updates on re-render).
  const loadingRef    = useRef(false);
  // Refs so requestChunk can always use the current path/sourceId without
  // being recreated on navigation (which would tear down and recreate the WS).
  const curPathRef      = useRef(initPath);
  const curSourceIdRef  = useRef(initSourceId);
  const filenameRef   = useRef(initFilename);

  // 1-based file line number of the first raw line in the current buffer.
  // null = unknown (after a scrollbar jump to an arbitrary byte offset).
  const [dosMode, setDosMode] = useState(false);
  const [wordWrap, setWordWrap] = useState(false);

  // ── Find state ────────────────────────────────────────────────────────────
  const [findOpen, setFindOpen]   = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [findIdx, setFindIdx]     = useState(0);
  const findInputRef  = useRef<HTMLInputElement>(null);
  // Stable refs so the keyboard handler closure doesn't go stale
  const findOpenRef    = useRef(false);
  const findMatchesRef = useRef<{ line: number; start: number; end: number }[]>([]);

  const [bufferBaseLine, setBufferBaseLine] = useState<number | null>(1);
  // Synchronous mirror of the current buffer's byte range — used by
  // scrollByLines so it always computes the next chunk offset from the
  // *latest* buffer, not a stale React-state closure.
  const bufRangeRef = useRef<{ offset: number; end: number; eof: boolean }>({
    offset: 0, end: 0, eof: false,
  });

  filenameRef.current    = curFilename;
  curPathRef.current     = curPath;
  curSourceIdRef.current = curSourceId;
  findOpenRef.current    = findOpen;
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
    let raw = (dosMode ? renderCp437(buffer) : renderText(buffer)).split("\n");
    // A buffer that ends with '\n' produces a trailing empty string — drop it
    // so the viewer doesn't show a phantom blank line at the chunk boundary.
    if (raw.length > 1 && raw[raw.length - 1] === "") raw.pop();
    const lines: string[] = [];
    const lineNums: (number | null)[] = [];
    // bufferBaseLine = null means we jumped to an unknown position
    const base = bufferBaseLine;
    for (let i = 0; i < raw.length; i++) {
      const line = raw[i];
      const fileLineNum = base !== null ? base + i : null;
      // When word-wrap is on the browser handles visual wrapping — keep each
      // logical line as a single entry so the gutter stays aligned.
      if (wordWrap || line.length <= MAX_COL) {
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
  }, [buffer, bufferOffset, effectiveMode, bufferBaseLine, dosMode, wordWrap]);

  const maxTopLine = Math.max(0, lines.length - vpLines);

  // ── Find matches (search across currently loaded lines) ───────────────────
  const findMatches = useMemo(() => {
    if (!findOpen || !findQuery) return [] as { line: number; start: number; end: number }[];
    const q = findQuery.toLowerCase();
    const result: { line: number; start: number; end: number }[] = [];
    for (let i = 0; i < lines.length; i++) {
      const lower = lines[i].toLowerCase();
      let pos = 0;
      let idx: number;
      while ((idx = lower.indexOf(q, pos)) !== -1) {
        result.push({ line: i, start: idx, end: idx + q.length });
        pos = idx + q.length;
      }
    }
    return result;
  }, [findOpen, findQuery, lines]);
  findMatchesRef.current = findMatches;

  // ── Request chunk from BE ─────────────────────────────────────────────────

  const requestChunk = useCallback((off: number, size: number, after: "top" | "bottom" = "top") => {
    afterLoadRef.current = after;
    loadingRef.current = true;          // synchronous guard for same-cycle callers
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

        bufRangeRef.current = { offset: m.offset, end: m.offset + bytes.length, eof: m.eof };
        // For small files baseLine is absolute (always starts at offset 0).
        // For large files the numbers are chunk-relative — the byte offset
        // shown in the status bar gives the positional context.
        setBufferBaseLine(1);

        if (m.fileSize !== undefined) setFileSize(m.fileSize);
        setBuffer(bytes);
        setBufferOffset(m.offset);
        loadingRef.current = false;
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
        setDosMode(false);
        setCurPath(m.path);
        setCurSourceId(m.sourceId);
        setCurFilename(m.filename);
        setUseDirect(direct);
        setCanDirect(direct);
        setMode(null);
        setBuffer(null);
        setBufferOffset(0);
        setIsSmall(null);
        setTopLine(0);
        setFileSize(null);
        setError(null);
        setNavError(null);
        loadingRef.current = !direct;
        setLoading(!direct);
        setBufferBaseLine(1);
        bufRangeRef.current = { offset: 0, end: 0, eof: false };
        setBufSeq(0);
        if (!direct) {
          // Request first chunk of new file
          const initSize = isImageExt(m.filename) ? IMAGE_LIMIT : SMALL_LIMIT;
          clientRef.current?.sendIntent("ReadFileChunk", undefined, {
            path: m.path, sourceId: m.sourceId, offset: 0, size: initSize,
          });
        }
      }
      if (msg.type === "ViewerPrefetchHints") {
        const m = msg as import("../ws/types").ViewerPrefetchHintsMessage;
        const fsPort = new URLSearchParams(window.location.search).get("fsport") ?? "0";
        for (const hint of m.hints) {
          if (canUseDirect(hint.isLocal, hint.filename, hint.fileSize ?? null)) {
            const img = new Image();
            img.src = `http://127.0.0.1:${fsPort}${hint.path}`;
          }
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
      // Read the synchronous buffer range — immune to stale React closures.
      const br = bufRangeRef.current;
      // Hit bottom edge of buffer — need next chunk?
      if (next >= maxTopLine && delta > 0 && !br.eof && !loadingRef.current && br.end > br.offset) {
        if (fileSize === null || br.end < fileSize) {
          requestChunk(br.end, CHUNK_SIZE, "top");
          return prev; // keep current until new buffer arrives
        }
      }
      // Hit top edge — need previous chunk?
      if (next <= 0 && delta < 0 && br.offset > 0 && !loadingRef.current) {
        const prevOff = Math.max(0, br.offset - CHUNK_SIZE);
        // Request exactly up to the current buffer start so the backend's
        // newline-aligned truncation keeps sequential continuity.
        requestChunk(prevOff, br.offset - prevOff, "bottom");
        return prev;
      }
      return next;
    });
  }, [maxTopLine, fileSize, requestChunk]);

  // ── Find helpers ─────────────────────────────────────────────────────────

  const findNext = useCallback(() => {
    setFindIdx(prev =>
      findMatchesRef.current.length === 0 ? 0 :
      (prev + 1) % findMatchesRef.current.length
    );
  }, []);

  const findPrev = useCallback(() => {
    setFindIdx(prev =>
      findMatchesRef.current.length === 0 ? 0 :
      (prev - 1 + findMatchesRef.current.length) % findMatchesRef.current.length
    );
  }, []);

  // Clamp findIdx when match count changes
  useEffect(() => {
    setFindIdx(prev => findMatches.length === 0 ? 0 : Math.min(prev, findMatches.length - 1));
  }, [findMatches.length]);

  // Scroll to active match
  useEffect(() => {
    if (!findOpen || findMatches.length === 0) return;
    const match = findMatches[Math.min(findIdx, findMatches.length - 1)];
    if (!match) return;
    if (isSmall === false) {
      // Large file: update the virtual scroll window to include the match
      setTopLine(clamp(match.line - Math.floor(vpLines / 2), 0, maxTopLine));
    }
    // For small files (and after the topLine update re-renders the lines for large
    // files), scroll the active <mark> into view. requestAnimationFrame ensures the
    // DOM has been updated with the new active-match class before we query it.
    requestAnimationFrame(() => {
      document.querySelector(".viewer__find-match--active")
        ?.scrollIntoView({ block: "center", inline: "nearest" });
    });
  }, [findIdx, findMatches, findOpen, isSmall, vpLines, maxTopLine]); // eslint-disable-line react-hooks/exhaustive-deps

  // Focus input when find opens, close when switching to non-text mode
  useEffect(() => {
    if (findOpen) setTimeout(() => findInputRef.current?.focus(), 0);
  }, [findOpen]);

  useEffect(() => {
    if (effectiveMode !== "text") setFindOpen(false);
  }, [effectiveMode]);

  // ── Keyboard ──────────────────────────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      const inFind = e.target === findInputRef.current;

      // F — open find (or previous match with Shift+F)
      if ((e.key === "f" || e.key === "F") && !ctrl && !e.altKey) {
        e.preventDefault();
        setFindOpen(true);
        if (e.shiftKey) findPrev();
        setTimeout(() => findInputRef.current?.focus(), 0);
        return;
      }

      // When typing in the find input, only intercept Escape and Enter
      if (inFind) {
        if (e.key === "Escape") { setFindOpen(false); }
        else if (e.key === "Enter") { e.preventDefault(); e.shiftKey ? findPrev() : findNext(); }
        return;
      }

      // F4 / E — open in preferred editor (guarded: must not be in find input or archive)
      if (e.key === "F4" || (e.key === "e" && !ctrl && !e.shiftKey && !e.altKey)) {
        if (!curSourceIdRef.current.startsWith("zip:") && !curSourceIdRef.current.startsWith("7z:")) {
          // curPathRef.current is the full file path (e.g. /home/user/docs/file.txt).
          // edit_file expects path=directory + filename, so strip the filename.
          const fullFilePath = curPathRef.current;
          const sep = fullFilePath.lastIndexOf("/");
          const dirPath = sep > 0 ? fullFilePath.slice(0, sep) : fullFilePath;
          clientRef.current?.sendIntent("EditFile", undefined, {
            path: dirPath, filename: filenameRef.current,
          });
        }
        return;
      }

      // Escape: close find bar first, otherwise close viewer
      if (e.key === "Escape") {
        if (findOpenRef.current) { setFindOpen(false); return; }
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

      if (e.key === "s" && effectiveMode === "text") { setDosMode(prev => !prev); return; }
      if (e.key === "w" && effectiveMode === "text") { setWordWrap(prev => !prev); return; }

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
          if (bufRangeRef.current.offset === 0) { setTopLine(0); }
          else if (!loadingRef.current) { requestChunk(0, CHUNK_SIZE, "top"); }
          return;
        }
        if (goEnd) {
          e.preventDefault();
          if (bufRangeRef.current.eof) { setTopLine(maxTopLine); }
          else if (!loadingRef.current && fileSize !== null) {
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
  }, [isSmall, fileSize, maxTopLine, vpLines, scrollByLines, requestChunk, windowLabel, effectiveMode, dosMode, findNext, findPrev]);

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
    if (fileSize === null || fileSize === 0 || loadingRef.current) return;
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
  }, [fileSize, buffer, bufferOffset, lines.length, maxTopLine, requestChunk]);

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
    if (effectiveMode !== "text" || !buffer) {
      return { gutterText: "", textContent: "", rows: [] as { gutter: string; content: string }[], visLines: [] as string[] };
    }
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
    // Large-file chunks show "+N" (relative to chunk start); small files show absolute "N".
    const prefix = isSmall === false ? "+" : "";
    const w = Math.max(4, (prefix + lastKnown).length);
    // Show file line number only on the first display row of each file line
    const gutterEntries = visNums.map((num, i) => {
      const isFirst = i === 0 || visNums[i - 1] !== num;
      if (!isFirst) return " ".repeat(w);
      if (num === null) return "~".repeat(w); // unknown position after a jump
      return (prefix + num).padStart(w);
    });
    // Per-line rows used by the word-wrap renderer — each logical line becomes
    // its own DOM row so the gutter number stays aligned to the first visual
    // row even when the content wraps across multiple lines.
    const rows = wordWrap
      ? visLines.map((content, i) => ({ gutter: gutterEntries[i], content }))
      : [];
    return { gutterText: gutterEntries.join("\n"), textContent: visLines.join("\n"), rows, visLines };
  }, [effectiveMode, buffer, isSmall, lines, lineNums, topLine, vpLines, wordWrap]);

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

  // These hooks must live before any early return (Rules of Hooks).
  const findLineFirstIdx = useMemo(() => {
    const map = new Map<number, number>();
    let g = 0;
    for (const m of findMatches) {
      if (!map.has(m.line)) map.set(m.line, g);
      g++;
    }
    return map;
  }, [findMatches]);

  const visStart = isSmall === true ? 0 : clamp(topLine, 0, lines.length);

  const findLabel = findMatches.length === 0
    ? (findQuery ? "No matches" : "")
    : isSmall === false
      ? `${Math.min(findIdx + 1, findMatches.length)}`
      : `${Math.min(findIdx + 1, findMatches.length)} / ${findMatches.length}`;

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
    <div className={`viewer${dosMode && effectiveMode === "text" ? " viewer--dos" : ""}`}>
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
          {effectiveMode === "text" && (
            <>
              <button
                className={`viewer__mode-btn${dosMode ? " viewer__mode-btn--active" : ""}`}
                onClick={() => setDosMode(prev => !prev)}
                title="DOS/CP437 mode (s)"
              >
                S DOS
              </button>
              <button
                className={`viewer__mode-btn${wordWrap ? " viewer__mode-btn--active" : ""}`}
                onClick={() => setWordWrap(prev => !prev)}
                title="Word wrap (w)"
              >
                W Wrap
              </button>
            </>
          )}
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
            ) : wordWrap ? (
              <div
                className={`viewer__text viewer__text--wrap${isSmall === false ? " viewer__text--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLDivElement>}
              >
                {textView.rows.map((row, i) => (
                  <div className="viewer__wrap-row" key={i}>
                    <span className="viewer__gutter-cell">{row.gutter}</span>
                    <span className="viewer__content-cell">
                      {(findOpen && findQuery)
                        ? highlightLine(row.content, findMatches, visStart + i, findIdx, findLineFirstIdx.get(visStart + i) ?? 0)
                        : (row.content || " ")}
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <div
                className={`viewer__text${isSmall === false ? " viewer__text--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLDivElement>}
              >
                <pre className="viewer__gutter" aria-hidden="true">
                  {textView.gutterText}
                </pre>
                <pre className="viewer__content">
                  {(findOpen && findQuery)
                    ? textView.visLines.map((line, i) => (
                        <React.Fragment key={i}>
                          {highlightLine(line, findMatches, visStart + i, findIdx, findLineFirstIdx.get(visStart + i) ?? 0)}
                          {i < textView.visLines.length - 1 ? "\n" : ""}
                        </React.Fragment>
                      ))
                    : textView.textContent}
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

      {findOpen && effectiveMode === "text" && (
        <div className="viewer__find-bar">
          <input
            ref={findInputRef}
            type="text"
            className="viewer__find-input"
            placeholder="Find…"
            value={findQuery}
            onChange={e => { setFindQuery(e.target.value); setFindIdx(0); }}
            spellCheck={false}
          />
          <span className="viewer__find-count">{findLabel}</span>
          <button className="viewer__find-nav" onClick={findPrev} title="Previous (Shift+Enter)">↑</button>
          <button className="viewer__find-nav" onClick={findNext} title="Next (Enter)">↓</button>
          <button className="viewer__find-close" onClick={() => setFindOpen(false)} title="Close (Esc)">×</button>
          {isSmall === false && (
            <span className="viewer__find-scope">searching loaded content</span>
          )}
        </div>
      )}

      <div className="viewer__status">
        <span>Offset: 0x{offsetDisplay.toString(16).padStart(8, "0")}</span>
        {fileSize !== null && <span>Size: {formatSize(fileSize)}</span>}
        <span>{pct}%</span>
        {loading && <span style={{ color: "var(--accent)" }}>loading…</span>}
        <span className="viewer__status-nav">
          {(() => {
            const canEdit = !curSourceId.startsWith("zip:") && !curSourceId.startsWith("7z:");
            const editHint = canEdit ? " · e/F4 edit" : "";
            if (isImageMode) return `z zoom · j/k prev/next file · 1 text · 2 image · 3 hex${editHint}`;
            if (effectiveMode === "text") return `↑↓ PgDn/PgUp scroll · j/k prev/next · 1 text · 2 image · 3 hex · s dos · w wrap · f find${editHint}`;
            return `↑↓ PgDn/PgUp scroll · j/k prev/next file · 1 text · 2 image · 3 hex${editHint}`;
          })()}
        </span>
      </div>
    </div>
  );
}
