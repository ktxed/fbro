import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IconArrowsMaximize } from "@tabler/icons-react";
import { resolveWsUrl, WsClient } from "../ws/client";
import type { BEMessage, FileChunkMessage, MarkdownPreviewMessage, ViewerFileChangedMessage, ViewerFindMatch, ViewerFindResultsMessage, StateSnapshotMessage, StatePatchMessage } from "../ws/types";
import { applyZoom, nextZoomSize, DEFAULT_FONT_SIZE } from "../utils/zoom";
import { ImageViewer, IMAGE_LIMIT } from "../viewers/ImageViewer";
import { ViewerLoading } from "../viewers/ViewerLoading";
import { detectRenderer, isImageExt, isMdExt, isVideoExt } from "../viewers/media";
import { DirectFileViewer, canUseDirect } from "../viewers/DirectFileViewer";
import { isArchiveSourceId, isRemoteSourceId } from "../utils/sourceId";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { DEFAULT_COLOR_SCHEME_ID, resolveColorScheme, type ColorSchemeId } from "../theme/colorSchemes";
import { buildMermaidBlock, mermaidSvgOf, setMermaidScale, stepMermaidScale, wheelZoomMermaid, type MermaidAction } from "../viewers/mermaidZoom";
import { MermaidOverlay, type MermaidOverlayHandle } from "../viewers/MermaidOverlay";
import { isLetterKey } from "./modalKeyboard";

// Mermaid v11 lazily registers diagram modules on the first render() call —
// initialize() alone is not enough, and the first real render can produce empty
// output before the registration promise settles. We do a discarded warm-up
// render here so every render() the viewer issues is on a fully-ready instance.
let _mermaidReady: Promise<Awaited<typeof import("mermaid")>["default"]> | null = null;
let _mermaidTheme: "dark" | "default" | null = null;
function getMermaid(theme: "dark" | "default") {
  if (!_mermaidReady) {
    _mermaidReady = import("mermaid").then(async ({ default: m }) => {
      m.initialize({ startOnLoad: false, theme, securityLevel: "strict", flowchart: { htmlLabels: false } });
      _mermaidTheme = theme;
      try { await m.render("mermaid-warmup", "graph TD\n  A-->B"); } catch {}
      return m;
    });
  }
  return _mermaidReady.then((m) => {
    if (_mermaidTheme !== theme) {
      m.initialize({ startOnLoad: false, theme, securityLevel: "strict", flowchart: { htmlLabels: false } });
      _mermaidTheme = theme;
    }
    return m;
  });
}

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
  initialColorScheme?: string | null;
  initialFindQuery?: string | null;
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

function displayRowAtByteOffset(
  bytes: Uint8Array,
  relativeOffset: number,
  dosMode: boolean,
  wordWrap: boolean,
): number {
  const prefix = dosMode
    ? renderCp437(bytes.slice(0, relativeOffset))
    : renderText(bytes.slice(0, relativeOffset));
  const logicalLines = prefix.split("\n");
  if (wordWrap) return logicalLines.length - 1;

  let displayRow = 0;
  for (const completeLine of logicalLines.slice(0, -1)) {
    displayRow += Math.max(1, Math.ceil(completeLine.length / MAX_COL));
  }
  displayRow += Math.floor(logicalLines[logicalLines.length - 1].length / MAX_COL);
  return displayRow;
}

function sanitizedMermaidSvg(svg: string): SVGElement | null {
  // Mermaid serializes HTML inside foreignObject using HTML void-element
  // syntax. In particular, state-diagram notes turn Markdown line breaks into
  // <br>, which is valid HTML but leaves the element open when parsed as XML.
  // Keep strict SVG/XML parsing by canonicalizing those breaks first.
  const xmlSvg = svg.replace(/<br(\s[^>]*)?>/gi, tag =>
    tag.endsWith("/>") ? tag : `${tag.slice(0, -1)}/>`
  );
  const doc = new DOMParser().parseFromString(xmlSvg, "image/svg+xml");
  if (doc.querySelector("parsererror")) return null;
  // Mermaid 11 renders flowchart node captions as sanitized HTML inside
  // foreignObject. Keep those containers or every node is left blank; their
  // descendants still pass through the attribute and active-content filters
  // below before the SVG is inserted into the viewer.
  doc.querySelectorAll("script,iframe,object,embed,image,a").forEach(node => node.remove());
  doc.querySelectorAll<HTMLElement>("*").forEach(element => {
    for (const attr of Array.from(element.attributes)) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on")) element.removeAttribute(attr.name);
      if ((name === "href" || name === "xlink:href") && !attr.value.startsWith("#")) {
        element.removeAttribute(attr.name);
      }
      if (name === "style") {
        const residue = attr.value.replace(/url\(\s*(['"]?)#[\w:.-]+\1\s*\)/gi, "");
        if (/url\s*\(|@import/i.test(residue)) element.removeAttribute(attr.name);
      }
    }
  });
  doc.querySelectorAll("style").forEach(style => {
    const residue = (style.textContent ?? "").replace(/url\(\s*(['"]?)#[\w:.-]+\1\s*\)/gi, "");
    if (/url\s*\(|@import/i.test(residue)) style.remove();
  });
  return doc.documentElement instanceof SVGElement ? doc.documentElement : null;
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

/** True when the selection lives inside an editable field (find bar, etc.). */
function selectionInEditable(sel: Selection): boolean {
  const node = sel.anchorNode;
  if (!node) return false;
  const el = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
  return !!el?.closest("input, textarea, [contenteditable='true']");
}

/**
 * Select only the textual file contents inside the viewer content root
 * (never chrome, gutters, or other UI).
 */
function selectAllViewerText(root: HTMLElement | null): boolean {
  if (!root) return false;
  const sel = window.getSelection();
  if (!sel) return false;

  const content = root.matches(".viewer__content")
    ? root
    : root.querySelector<HTMLElement>(".viewer__content");
  if (content) {
    const range = document.createRange();
    range.selectNodeContents(content);
    sel.removeAllRanges();
    sel.addRange(range);
    return true;
  }

  const cells = root.querySelectorAll(".viewer__content-cell");
  if (cells.length > 0) {
    const first = cells[0];
    const last = cells[cells.length - 1];
    const range = document.createRange();
    range.setStart(first, 0);
    range.setEnd(last, last.childNodes.length);
    sel.removeAllRanges();
    sel.addRange(range);
    return true;
  }

  // Hex dump <pre> or markdown preview root — the ref points at the content itself.
  if (root.matches(".viewer__pre, .viewer__markdown")) {
    const range = document.createRange();
    range.selectNodeContents(root);
    sel.removeAllRanges();
    sel.addRange(range);
    return true;
  }

  return false;
}

/**
 * Plain text for the current selection inside the viewer content root.
 * Wrap mode stores one line per cell (no text newlines in the DOM), so we
 * re-join selected cells with "\n".
 */
function selectedViewerPlainText(root: HTMLElement, sel: Selection): string {
  if (!root.classList.contains("viewer__text--wrap") || sel.rangeCount === 0) {
    return sel.toString();
  }
  const range = sel.getRangeAt(0);
  const parts: string[] = [];
  root.querySelectorAll(".viewer__content-cell").forEach((cell) => {
    try {
      if (!range.intersectsNode(cell)) return;
      const cellRange = document.createRange();
      cellRange.selectNodeContents(cell);
      const clipped = cellRange.cloneRange();
      if (range.compareBoundaryPoints(Range.START_TO_START, cellRange) > 0) {
        clipped.setStart(range.startContainer, range.startOffset);
      }
      if (range.compareBoundaryPoints(Range.END_TO_END, cellRange) < 0) {
        clipped.setEnd(range.endContainer, range.endOffset);
      }
      parts.push(clipped.toString());
    } catch {
      // Range boundaries can be transient mid-update; skip that cell.
    }
  });
  return parts.length > 0 ? parts.join("\n") : sel.toString();
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

export function ViewerApp({ path: initPath, sourceId: initSourceId, filename: initFilename, windowLabel, panelId, isLocal, nativeFileSize, initialColorScheme, initialFindQuery }: ViewerAppProps) {
  // Effective image cap (in bytes) for this viewer window. Passed via the
  // launch URL by the BE based on the current ludicrousMode + maxImageSizeMb.
  // Falls back to IMAGE_LIMIT (5 MiB) so old links / missing param keep working.
  const effectiveImageLimit = (() => {
    const raw = new URLSearchParams(window.location.search).get("imgcap");
    if (!raw) return IMAGE_LIMIT;
    const n = Number(raw);
    return Number.isFinite(n) && n > 0 ? n : IMAGE_LIMIT;
  })();

  // Current file being viewed (updated on navigation)
  const [curPath, setCurPath]         = useState(initPath);
  const [curSourceId, setCurSourceId] = useState(initSourceId);
  const [curFilename, setCurFilename] = useState(initFilename);
  const [curIsLocal, setCurIsLocal]   = useState(isLocal);

  const [mode, setMode]           = useState<ViewMode | null>(null);
  const [fileSize, setFileSize]   = useState<number | null>(null);
  const [loading, setLoading]     = useState(true);
  const [error, setError]         = useState<string | null>(null);
  const [navError, setNavError]   = useState<string | null>(null);
  const [pendingNavigation, setPendingNavigation] = useState(0);
  const [colorSchemeId, setColorSchemeId] = useState<ColorSchemeId>(() => resolveColorScheme(initialColorScheme ?? DEFAULT_COLOR_SCHEME_ID).id);

  // Buffer
  const [buffer, setBuffer]             = useState<Uint8Array | null>(null);
  const [bufferOffset, setBufferOffset] = useState(0);
  // eof is tracked via bufRangeRef (synchronous) rather than React state.

  // null = undetermined, true = small file, false = large file
  const [isSmall, setIsSmall] = useState<boolean | null>(null);

  // true = use file:// direct rendering (local images < imageLimit, video, PDF)
  const [useDirect, setUseDirect] = useState(() => canUseDirect(isLocal, initFilename, nativeFileSize, effectiveImageLimit));
  // true = current file supports direct rendering (even if user switched to text/hex)
  const [canDirect, setCanDirect] = useState(() => canUseDirect(isLocal, initFilename, nativeFileSize, effectiveImageLimit));

  // Large-file virtual scroll: line index at the top of viewport
  const [topLine, setTopLine] = useState(0);
  // Viewport height in lines
  const [vpLines, setVpLines] = useState(30);
  // Incremented each time a new buffer arrives — drives post-load effects
  const [bufSeq, setBufSeq]   = useState(0);

  const clientRef          = useRef<WsClient | null>(null);
  const currentFontSizeRef = useRef(DEFAULT_FONT_SIZE);
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
  const curIsLocalRef = useRef(isLocal);
  const videoRef      = useRef<HTMLVideoElement | null>(null);

  // 1-based file line number of the first raw line in the current buffer.
  // null = unknown (after a scrollbar jump to an arbitrary byte offset).
  const [dosMode, setDosMode] = useState(false);
  const [wordWrap, setWordWrap] = useState(false);
  const [markdownMode, setMarkdownMode] = useState(() => isMdExt(initFilename));
  const [markdownPreview, setMarkdownPreview] = useState<MarkdownPreviewMessage | null>(null);

  // ── Find state ────────────────────────────────────────────────────────────
  const [findOpen, setFindOpen]   = useState(false);
  const [findQuery, setFindQuery] = useState(() => initialFindQuery ?? "");
  const [findIdx, setFindIdx]     = useState(0);
  const [largeFindMatches, setLargeFindMatches] = useState<ViewerFindMatch[]>([]);
  const [largeFindPending, setLargeFindPending] = useState(false);
  const [largeFindTruncated, setLargeFindTruncated] = useState(false);
  const [largeFindError, setLargeFindError] = useState<string | null>(null);
  const findHistory = useInputHistory("viewer.find", setFindQuery);
  // Stable refs so the keyboard handler closure doesn't go stale
  const findOpenRef    = useRef(false);
  const findMatchesRef = useRef<{ line: number; start: number; end: number }[]>([]);
  const largeFindMatchesRef = useRef<ViewerFindMatch[]>([]);
  const findRequestIdRef = useRef("");
  const pendingFindLineRef = useRef<ViewerFindMatch | null>(null);

  // ── Expanded mermaid diagram (full-viewer zoom/pan overlay) ───────────────
  const [expandedMermaid, setExpandedMermaid] = useState<SVGSVGElement | null>(null);
  const expandedMermaidRef = useRef<SVGSVGElement | null>(null);
  const mermaidOverlayRef = useRef<MermaidOverlayHandle>(null);

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
  curIsLocalRef.current  = curIsLocal;
  findOpenRef.current    = findOpen;
  expandedMermaidRef.current = expandedMermaid;
  largeFindMatchesRef.current = largeFindMatches;
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

  function toggleMaximize() {
    try {
      void getCurrentWindow().toggleMaximize().catch((cause) => {
        console.error("Could not toggle viewer maximize:", cause);
      });
    } catch (cause) {
      // Browser-based tests do not expose a Tauri window.
      console.error("Could not toggle viewer maximize:", cause);
    }
  }

  useEffect(() => {
    const title = `${curFilename} — fbro viewer`;
    // Keep the document title in sync for browser-based runs and accessibility;
    // Tauri's native title must be updated explicitly after viewer navigation.
    document.title = title;
    try {
      void getCurrentWindow().setTitle(title).catch(() => {});
    } catch {
      // Browser-based tests do not expose a Tauri window.
    }
  }, [curFilename]);

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

  // Large files are searched by the backend through the active VFS driver.
  // Debouncing avoids scanning on every keystroke; request IDs make results
  // from superseded scans harmless even though their I/O may still finish.
  useEffect(() => {
    if (!findOpen || !findQuery || isSmall !== false || effectiveMode !== "text") {
      findRequestIdRef.current = "";
      setLargeFindMatches([]);
      setLargeFindPending(false);
      setLargeFindTruncated(false);
      setLargeFindError(null);
      return;
    }
    const requestId = globalThis.crypto?.randomUUID?.() ?? `viewer-find-${Date.now()}-${Math.random()}`;
    findRequestIdRef.current = requestId;
    setLargeFindMatches([]);
    setLargeFindPending(true);
    setLargeFindTruncated(false);
    setLargeFindError(null);
    setFindIdx(0);
    const timer = window.setTimeout(() => {
      clientRef.current?.sendIntent("FindInViewerFile", undefined, {
        requestId,
        path: curPathRef.current,
        sourceId: curSourceIdRef.current,
        query: findQuery,
      });
    }, 180);
    return () => window.clearTimeout(timer);
  }, [findOpen, findQuery, isSmall, effectiveMode, curPath, curSourceId]);

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

  // ── Initial load via HTTP file server ────────────────────────────────────
  //
  // For small local files we fetch the content directly from the HTTP file
  // server instead of waiting for the WebSocket handshake. On macOS a freshly
  // created WKWebView takes ~1s before its outgoing WebSocket connections
  // succeed, but its HTTP stack is ready immediately — so an `<img src=…>` or
  // `fetch()` to localhost is instant. The prefetch cache (warmed when the
  // viewer was opened) means the file server usually serves this from memory.

  const httpInitDoneRef = useRef(false);

  useEffect(() => {
    if (!isLocal) return;
    if (canUseDirect(isLocal, initFilename, nativeFileSize, effectiveImageLimit)) return;
    if (nativeFileSize === null || nativeFileSize > SMALL_LIMIT) return;

    const fsPort = Number(new URLSearchParams(window.location.search).get("fsport") ?? "0");
    if (!fsPort) return;

    const urlPath = initPath.startsWith("/") ? initPath : `/${initPath}`;
    const url = `http://127.0.0.1:${fsPort}${urlPath}`;

    let cancelled = false;
    fetch(url).then(async (response) => {
      if (cancelled || !response.ok) return;
      const ab = await response.arrayBuffer();
      if (cancelled) return;
      const bytes = new Uint8Array(ab);

      httpInitDoneRef.current = true;
      bufRangeRef.current = { offset: 0, end: bytes.length, eof: true };
      setBufferBaseLine(1);
      setFileSize(nativeFileSize);
      setBuffer(bytes);
      setBufferOffset(0);
      loadingRef.current = false;
      setLoading(false);
      setMode(prev => {
        if (prev !== null) return prev;
        const detected = detectRenderer(filenameRef.current, detectBinary(bytes));
        if (detected === "image" && nativeFileSize > effectiveImageLimit) return "hex";
        return detected;
      });
      setNavError(null);
      setIsSmall(prev => prev !== null ? prev : bytes.length <= SMALL_LIMIT);
      setBufSeq(s => s + 1);
    }).catch(() => {
      // Fall through to the WS path on any error.
    });

    return () => { cancelled = true; };
  }, []); // run once on mount; navigation handles its own loads

  // ── WS connection ─────────────────────────────────────────────────────────

  useEffect(() => {
    let cancelled = false;
    resolveWsUrl().then((wsUrl) => {
      if (cancelled) return;
      const client = new WsClient("viewer", wsUrl);
      clientRef.current = client;
      client.onDisconnected = () => {
        setPendingNavigation(0);
        setError("Viewer connection lost; reconnecting…");
      };

      client.onConnected = () => {
        setError(null);
        if (httpInitDoneRef.current) {
          // The HTTP fast path supplies raw bytes only. Markdown must still
          // cross the backend renderer so the viewer receives sanitized HTML
          // and its prevalidated local-link table.
          if (isMdExt(filenameRef.current)) requestChunk(0, SMALL_LIMIT);
          return;
        }
        if (canUseDirect(isLocal, initFilename, nativeFileSize, effectiveImageLimit)) {
          setLoading(false);
        } else {
          // For known image types, request up to the effective image limit in one shot
          const initSize = isImageExt(initFilename) ? effectiveImageLimit : SMALL_LIMIT;
          requestChunk(0, initSize);
        }
      };

      client.onMessage = (msg: BEMessage) => {
        if (msg.type === "StateSnapshot") {
          const snapshot = msg as StateSnapshotMessage;
          const fontSize = snapshot.domains.fontSize;
          if (fontSize !== undefined) { currentFontSizeRef.current = fontSize; applyZoom(fontSize); }
          setColorSchemeId(resolveColorScheme(snapshot.domains.settings?.general.colorScheme).id);
        }
        if (msg.type === "StatePatch") {
          const patch = msg as StatePatchMessage;
          if (patch.domain === "settings" && patch.patch.fontSize !== undefined) {
            const fontSize = patch.patch.fontSize as number;
            currentFontSizeRef.current = fontSize;
            applyZoom(fontSize);
          }
          if (patch.domain === "settings") {
            const general = patch.patch.general as { colorScheme?: string } | undefined;
            if (general?.colorScheme !== undefined) {
              setColorSchemeId(resolveColorScheme(general.colorScheme).id);
            }
          }
        }
        if (msg.type === "FileChunk") {
          const m = msg as FileChunkMessage;
          if (m.path !== curPathRef.current) return;
          const bytes = hexDecode(m.data);

        bufRangeRef.current = { offset: m.offset, end: m.offset + bytes.length, eof: m.eof };
        // For small files baseLine is absolute (always starts at offset 0).
        // For large files the numbers are chunk-relative — the byte offset
        // shown in the status bar gives the positional context.
        const pendingFindLine = pendingFindLineRef.current;
        if (pendingFindLine
          && pendingFindLine.byteOffset >= m.offset
          && pendingFindLine.byteOffset < m.offset + bytes.length) {
          const relativeOffset = pendingFindLine.byteOffset - m.offset;
          let precedingNewlines = 0;
          for (let i = 0; i < relativeOffset; i++) {
            if (bytes[i] === 10) precedingNewlines++;
          }
          setBufferBaseLine(Math.max(1, pendingFindLine.lineNumber - precedingNewlines));
          pendingFindLineRef.current = null;
        } else {
          setBufferBaseLine(m.offset === 0 ? 1 : null);
        }

        if (m.fileSize !== undefined) setFileSize(m.fileSize);
        setBuffer(bytes);
        setBufferOffset(m.offset);
        loadingRef.current = false;
        setLoading(false);
        setMode(prev => {
          if (prev !== null) return prev;
          const detected = detectRenderer(filenameRef.current, detectBinary(bytes));
          if (detected === "image" && m.fileSize !== undefined && m.fileSize > effectiveImageLimit) return "hex";
          return detected;
        });
        setNavError(null);
        setIsSmall(prev => prev !== null ? prev : (m.eof && bytes.length <= SMALL_LIMIT));
        setBufSeq(s => s + 1);
        }
        if (msg.type === "MarkdownPreview") {
          const m = msg as MarkdownPreviewMessage;
          if (m.path === curPathRef.current) setMarkdownPreview(m);
        }
        if (msg.type === "ViewerFindResults") {
          const m = msg as ViewerFindResultsMessage;
          if (m.requestId !== findRequestIdRef.current || m.path !== curPathRef.current) return;
          if (m.error) setLargeFindError(m.error);
          if (m.matches.length > 0) setLargeFindMatches(previous => [...previous, ...m.matches]);
          setLargeFindTruncated(m.truncated);
          if (m.done) setLargeFindPending(false);
        }
        if (msg.type === "ViewerFileChanged") {
          const m = msg as ViewerFileChangedMessage;
        setPendingNavigation(n => Math.max(0, n - 1));
        curPathRef.current = m.path;
        curSourceIdRef.current = m.sourceId;
        filenameRef.current = m.filename;
        const direct = canUseDirect(m.isLocal ?? false, m.filename, m.fileSize ?? null, effectiveImageLimit);
        // Reset viewer state for new file
        setDosMode(false);
        setMarkdownMode(isMdExt(m.filename));
        setMarkdownPreview(null);
        curPathRef.current = m.path;
        curSourceIdRef.current = m.sourceId;
        setCurPath(m.path);
        setCurSourceId(m.sourceId);
        setCurFilename(m.filename);
        setCurIsLocal(m.isLocal ?? false);
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
        setLargeFindMatches([]);
        setLargeFindPending(false);
        setLargeFindTruncated(false);
        setLargeFindError(null);
        findRequestIdRef.current = "";
        pendingFindLineRef.current = null;
        setFindIdx(0);
        bufRangeRef.current = { offset: 0, end: 0, eof: false };
        afterLoadRef.current = "top";
        pendingJumpRef.current = null;
        setBufSeq(0);
        if (!direct) {
          // If BE signals binary, pre-set hex mode and request only the initial
          // hex view chunk (64 KiB) rather than the full 1 MiB detection read.
          if (m.isBinary === true) {
            setMode("hex");
          }
          const initSize = m.isBinary === true
            ? CHUNK_SIZE
            : isImageExt(m.filename) ? effectiveImageLimit : SMALL_LIMIT;
          clientRef.current?.sendIntent("ReadFileChunk", undefined, {
            path: m.path, sourceId: m.sourceId, offset: 0, size: initSize,
          });
        }
        }
        if (msg.type === "Error") {
          const err = msg as { code: string; message: string };
          setPendingNavigation(n => Math.max(0, n - 1));
          if (err.code === "NOT_A_FILE") {
            setNavError(err.message);
          } else {
            setError(err.message ?? "Unknown error");
            setLoading(false);
          }
        }
      };

      client.connect();
    });
    return () => {
      cancelled = true;
      clientRef.current?.destroy();
      clientRef.current = null;
    };
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
    const count = isSmall === false ? largeFindMatchesRef.current.length : findMatchesRef.current.length;
    setFindIdx(prev => count === 0 ? 0 : (prev + 1) % count);
  }, [isSmall]);

  const findPrev = useCallback(() => {
    const count = isSmall === false ? largeFindMatchesRef.current.length : findMatchesRef.current.length;
    setFindIdx(prev => count === 0 ? 0 : (prev - 1 + count) % count);
  }, [isSmall]);

  // Clamp findIdx when match count changes
  const totalFindMatches = isSmall === false ? largeFindMatches.length : findMatches.length;
  useEffect(() => {
    setFindIdx(prev => totalFindMatches === 0 ? 0 : Math.min(prev, totalFindMatches - 1));
  }, [totalFindMatches]);

  // Scroll to active match
  useEffect(() => {
    if (!findOpen || isSmall === false || findMatches.length === 0) return;
    const match = findMatches[Math.min(findIdx, findMatches.length - 1)];
    if (!match) return;
    // requestAnimationFrame ensures the DOM has been updated with the new
    // active-match class before we query it.
    requestAnimationFrame(() => {
      document.querySelector(".viewer__find-match--active")
        ?.scrollIntoView({ block: "center", inline: "nearest" });
    });
  }, [findIdx, findMatches, findOpen, isSmall, vpLines, maxTopLine]); // eslint-disable-line react-hooks/exhaustive-deps

  // Load the byte window containing the selected backend match, then derive
  // its exact display row from the preceding bytes. Byte-percentage estimates
  // are not accurate enough when line lengths vary substantially.
  useEffect(() => {
    if (!findOpen || isSmall !== false || largeFindMatches.length === 0 || !buffer) return;
    const match = largeFindMatches[Math.min(findIdx, largeFindMatches.length - 1)];
    if (!match) return;
    const br = bufRangeRef.current;
    if (match.byteOffset >= br.offset && match.byteOffset < br.end) {
      const relativeOffset = match.byteOffset - br.offset;
      let precedingNewlines = 0;
      for (let i = 0; i < relativeOffset; i++) {
        if (buffer[i] === 10) precedingNewlines++;
      }
      setBufferBaseLine(Math.max(1, match.lineNumber - precedingNewlines));
      const matchRow = displayRowAtByteOffset(buffer, relativeOffset, dosMode, wordWrap);
      setTopLine(clamp(matchRow - Math.floor(vpLines / 2), 0, maxTopLine));
      return;
    }
    if (loadingRef.current || fileSize === null) return;
    const loadOff = clamp(
      Math.floor(match.byteOffset - CHUNK_SIZE / 3),
      0,
      Math.max(0, fileSize - CHUNK_SIZE),
    );
    pendingFindLineRef.current = match;
    pendingJumpRef.current = null;
    requestChunk(loadOff, CHUNK_SIZE, "top");
  }, [findIdx, largeFindMatches, findOpen, isSmall, buffer, fileSize, maxTopLine, vpLines, dosMode, wordWrap, requestChunk]);

  // Focus input when find opens, close when switching to non-text mode
  useEffect(() => {
    if (findOpen) setTimeout(() => (findHistory.inputRef as React.RefObject<HTMLInputElement>).current?.focus(), 0);
  }, [findOpen]);

  useEffect(() => {
    if (effectiveMode !== "text") setFindOpen(false);
  }, [effectiveMode]);

  useEffect(() => {
    if (markdownMode) setFindOpen(false);
  }, [markdownMode]);

  // ── Keyboard ──────────────────────────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      const inFind = e.target === findHistory.inputRef.current;

      if (ctrl && (e.key === "=" || e.key === "+")) {
        e.preventDefault();
        const s = nextZoomSize(currentFontSizeRef.current, 1);
        currentFontSizeRef.current = s;
        applyZoom(s);
        clientRef.current?.sendIntent("SetFontSize", undefined, { value: s });
        return;
      }
      if (ctrl && e.key === "-") {
        e.preventDefault();
        const s = nextZoomSize(currentFontSizeRef.current, -1);
        currentFontSizeRef.current = s;
        applyZoom(s);
        clientRef.current?.sendIntent("SetFontSize", undefined, { value: s });
        return;
      }
      if (ctrl && e.key === "0") {
        e.preventDefault();
        currentFontSizeRef.current = DEFAULT_FONT_SIZE;
        applyZoom(DEFAULT_FONT_SIZE);
        clientRef.current?.sendIntent("SetFontSize", undefined, { value: DEFAULT_FONT_SIZE });
        return;
      }

      // Expanded mermaid diagram owns the keyboard: Esc returns to the
      // document, and every other viewer shortcut is held back so the content
      // underneath can't change while it is covered.
      if (expandedMermaidRef.current) {
        const overlay = mermaidOverlayRef.current;
        const pan: Record<string, [number, number]> = {
          ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1],
        };
        if (e.key === "Escape") { e.preventDefault(); setExpandedMermaid(null); }
        else if (e.key === "+" || e.key === "=") { e.preventDefault(); overlay?.zoomIn(); }
        else if (e.key === "-") { e.preventDefault(); overlay?.zoomOut(); }
        else if (e.key === "0") { e.preventDefault(); overlay?.fit(); }
        else if (pan[e.key]) { e.preventDefault(); overlay?.panBy(...pan[e.key]); }
        else if (!ctrl && !e.altKey && isLetterKey(e, "m")) { e.preventDefault(); toggleMaximize(); }
        return;
      }

      // When typing in the find input, only intercept Escape and Enter
      if (inFind) {
        if (e.key === "Escape") { findHistory.addToHistory(findQuery); setFindOpen(false); }
        else if (e.key === "Enter") { e.preventDefault(); findHistory.addToHistory(findQuery); e.shiftKey ? findPrev() : findNext(); }
        return;
      }

      // F — open find (or previous match with Shift+F)
      if (isLetterKey(e, "f") && !ctrl && !e.altKey) {
        e.preventDefault();
        if (effectiveMode === "text" && markdownMode) setMarkdownMode(false);
        setFindOpen(true);
        if (e.shiftKey) findPrev();
        setTimeout(() => (findHistory.inputRef as React.RefObject<HTMLInputElement>).current?.focus(), 0);
        return;
      }

      // Ctrl/Cmd+A — select all textual file contents (fully loaded / small files only).
      // Always preventDefault so the browser never selects chrome (header, status, etc.).
      if (ctrl && isLetterKey(e, "a")) {
        e.preventDefault();
        if (isSmall === true && !isImageMode && contentRef.current) {
          selectAllViewerText(contentRef.current);
        }
        return;
      }

      // F4 / E — open in preferred editor only for real local files.
      if (e.key === "F4" || (isLetterKey(e, "e") && !ctrl && !e.shiftKey && !e.altKey)) {
        if (curIsLocalRef.current && !isArchiveSourceId(curSourceIdRef.current) && !isRemoteSourceId(curSourceIdRef.current)) {
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

      // Escape: close find bar first, otherwise close viewer.
      // On macOS, calling window.close() while the window is in native
      // fullscreen (green-button) crashes the app — the OS animates a
      // fullscreen-exit on a window being torn down. So when fullscreen,
      // Escape leaves fullscreen first (matching native behavior) and only
      // closes the viewer when windowed.
      if (e.key === "Escape") {
        if (findOpenRef.current) { setFindOpen(false); return; }
        const closeViewer = () =>
          clientRef.current?.sendIntent("CloseViewerWindow", undefined, { label: windowLabel });
        try {
          const appWindow = getCurrentWindow();
          appWindow.isFullscreen()
            .then((fs) => { if (fs) appWindow.setFullscreen(false); else closeViewer(); })
            .catch(closeViewer);
        } catch {
          closeViewer();
        }
        return;
      }

      if (e.key === "1") { setMarkdownMode(false); switchMode("text"); return; }
      if (e.key === "2") { switchMode("image"); return; }
      if (e.key === "3") { switchMode("hex"); return; }
      if (e.key === "4" && isMdExt(filenameRef.current)) {
        setMarkdownMode(true);
        switchMode("text");
        return;
      }

      if (!ctrl && !e.altKey && isLetterKey(e, "m")) {
        e.preventDefault();
        toggleMaximize();
        return;
      }

      // Space toggles play/pause for the directly-rendered <video>. The
      // window keydown handler runs before the video gets focus, so the
      // browser's native space-on-focused-video shortcut never fires.
      if (e.key === " " && videoRef.current && isVideoExt(filenameRef.current)) {
        e.preventDefault();
        const v = videoRef.current;
        if (v.paused) v.play().catch(() => {}); else v.pause();
        return;
      }
      if (isLetterKey(e, "k") && !e.shiftKey) {
        e.preventDefault();
        setNavError(null);
        if (clientRef.current) setPendingNavigation(n => n + 1);
        clientRef.current?.sendIntent("ViewerNavigate", undefined, { panelId, direction: "next" });
        return;
      }
      if (isLetterKey(e, "j") && !e.shiftKey) {
        e.preventDefault();
        setNavError(null);
        if (clientRef.current) setPendingNavigation(n => n + 1);
        clientRef.current?.sendIntent("ViewerNavigate", undefined, { panelId, direction: "prev" });
        return;
      }

      if (!ctrl && !e.altKey && isLetterKey(e, "s") && effectiveMode === "text") {
        setDosMode(prev => !prev);
        return;
      }
      if (!ctrl && !e.altKey && isLetterKey(e, "w") && effectiveMode === "text") {
        setWordWrap(prev => !prev);
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
  }, [isSmall, fileSize, maxTopLine, vpLines, scrollByLines, requestChunk, windowLabel, effectiveMode, dosMode, markdownMode, findNext, findPrev, isImageMode]);

  // Copy: only textual file contents (never chrome). Wrap mode re-joins lines.
  useEffect(() => {
    const onCopy = (e: ClipboardEvent) => {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return;
      if (selectionInEditable(sel)) return;

      const root = contentRef.current;
      const anchorIn = !!(root && sel.anchorNode && root.contains(sel.anchorNode));
      const focusIn = !!(root && sel.focusNode && root.contains(sel.focusNode));
      if (!root || !anchorIn || !focusIn) {
        // Selection is outside the content root (or mixed with chrome) — block.
        e.preventDefault();
        e.clipboardData?.setData("text/plain", "");
        return;
      }

      // Non-wrap text / hex / markdown: native sel.toString() is fine, but we
      // still set clipboard explicitly so nothing outside the root can leak in.
      const text = selectedViewerPlainText(root, sel);
      e.preventDefault();
      e.clipboardData?.setData("text/plain", text);
    };
    document.addEventListener("copy", onCopy);
    return () => document.removeEventListener("copy", onCopy);
  }, []);

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

  const isMarkdownPreviewActive = effectiveMode === "text" && markdownMode && isSmall === true;

  const markdownHtml = isMarkdownPreviewActive && markdownPreview?.path === curPath
    ? markdownPreview.html
    : "";

  // Start loading mermaid as soon as the viewer mounts on a markdown file, so
  // the async warm-up overlaps with the file fetch instead of running after.
  useEffect(() => {
    if (isMdExt(filenameRef.current)) getMermaid(resolveColorScheme(colorSchemeId).mermaidTheme);
  }, [colorSchemeId]);

  // An expanded diagram belongs to the document it was cloned from; drop it
  // when the preview is replaced (navigation, reload, theme) or left.
  useEffect(() => { setExpandedMermaid(null); }, [markdownHtml, colorSchemeId]);

  // Inject the markdown HTML manually and render any mermaid diagrams. We
  // deliberately avoid dangerouslySetInnerHTML: React would rewrite innerHTML
  // on every re-render (resize, font-size sync, mode detect), wiping the SVG
  // we replace the <pre> with.
  useEffect(() => {
    if (!markdownHtml || !contentRef.current) return;
    const container = contentRef.current;
    container.innerHTML = markdownHtml;
    const targets = new Map(markdownPreview?.links.map(link => [`#${link.id}`, link.path]) ?? []);
    container.querySelectorAll<HTMLAnchorElement>("a[href]").forEach(anchor => {
      const target = targets.get(anchor.getAttribute("href") ?? "");
      // Removing href blocks middle-click and modifier-click too; the viewer
      // handles backend-approved files itself instead of browser navigation.
      anchor.removeAttribute("href");
      if (target) anchor.dataset.viewerMarkdownTarget = target;
    });
    const handleClick = (event: MouseEvent) => {
      const element = event.target instanceof Element ? event.target : null;
      const button = element?.closest<HTMLButtonElement>("button[data-mermaid-action]");
      const block = button?.closest<HTMLElement>(".viewer__mermaid");
      if (button && block && container.contains(block)) {
        const action = button.dataset.mermaidAction as MermaidAction;
        if (action === "in") stepMermaidScale(block, 1);
        else if (action === "out") stepMermaidScale(block, -1);
        else if (action === "fit") setMermaidScale(block, "fit");
        else if (action === "expand") {
          const svg = mermaidSvgOf(block);
          if (svg) setExpandedMermaid(svg.cloneNode(true) as SVGSVGElement);
        }
        return;
      }
      const anchor = element?.closest<HTMLAnchorElement>("a[data-viewer-markdown-target]");
      if (!anchor || !container.contains(anchor)) return;

      // Never let the webview perform native URL navigation. The only links
      // supported by the viewer are relative paths to another Markdown file.
      event.preventDefault();
      const target = anchor.dataset.viewerMarkdownTarget;
      if (!target) return;
      clientRef.current?.sendIntent("ViewerOpenFile", undefined, {
        panelId,
        sourceId: curSourceIdRef.current,
        path: target,
      });
    };
    container.addEventListener("click", handleClick);
    // Ctrl/Cmd+wheel over a diagram zooms it; a plain wheel keeps scrolling
    // the document.
    const handleWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      const element = event.target instanceof Element ? event.target : null;
      const block = element?.closest<HTMLElement>(".viewer__mermaid-canvas")?.parentElement;
      if (!block || !container.contains(block)) return;
      event.preventDefault();
      wheelZoomMermaid(block, event, container);
    };
    container.addEventListener("wheel", handleWheel, { passive: false });
    const codeBlocks = Array.from(
      container.querySelectorAll<HTMLElement>("code.language-mermaid"),
    );

    let cancelled = false;
    if (codeBlocks.length > 0) getMermaid(resolveColorScheme(colorSchemeId).mermaidTheme).then(mermaid => {
      if (cancelled) return;
      codeBlocks.forEach(async (code, i) => {
        const source = code.textContent ?? "";
        const pre = code.parentElement;
        if (!pre || !pre.isConnected) return;
        try {
          const { svg } = await mermaid.render(`mermaid-${i}-${Date.now()}`, source);
          if (cancelled || !pre.isConnected) return;
          const safeSvg = sanitizedMermaidSvg(svg);
          if (!safeSvg) throw new Error("Mermaid returned invalid SVG");
          pre.replaceWith(buildMermaidBlock(document.importNode(safeSvg, true) as SVGSVGElement));
        } catch (err) {
          console.error("mermaid render failed:", err);
        }
      });
    });
    return () => {
      cancelled = true;
      container.removeEventListener("click", handleClick);
      container.removeEventListener("wheel", handleWheel);
    };
  }, [markdownHtml, markdownPreview, colorSchemeId, panelId]);

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

  let renderedFindIdx = findIdx;
  if (isSmall === false && largeFindMatches.length > 0) {
    const selected = largeFindMatches[Math.min(findIdx, largeFindMatches.length - 1)];
    const br = bufRangeRef.current;
    if (selected && selected.byteOffset >= br.offset && selected.byteOffset < br.end) {
      renderedFindIdx = largeFindMatches
        .slice(0, findIdx)
        .filter(match => match.byteOffset >= br.offset && match.byteOffset < br.end)
        .length;
    } else {
      renderedFindIdx = -1;
    }
  }

  const findLabel = isSmall === false
    ? largeFindError
      ? "Search failed"
      : largeFindMatches.length === 0
        ? (largeFindPending ? "Searching…" : (findQuery ? "No matches" : ""))
        : `${Math.min(findIdx + 1, largeFindMatches.length)} / ${largeFindMatches.length}${largeFindPending || largeFindTruncated ? "+" : ""} · line ${largeFindMatches[Math.min(findIdx, largeFindMatches.length - 1)].lineNumber}`
    : findMatches.length === 0
      ? (findQuery ? "No matches" : "")
      : `${Math.min(findIdx + 1, findMatches.length)} / ${findMatches.length}`;
  const colorScheme = resolveColorScheme(colorSchemeId);

  if (useDirect) {
    return (
      <div
        className="viewer"
        data-color-scheme={colorScheme.id}
        style={{ colorScheme: colorScheme.mode }}
      >
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
            <button
              className="viewer__mode-btn"
              onClick={toggleMaximize}
              title="Toggle maximize (M)"
              aria-label="Toggle maximize"
            >
              <IconArrowsMaximize size={14} aria-hidden /> M Max
            </button>
          </div>
        </div>
        <div className="viewer__body" ref={bodyRef}>
          {pendingNavigation > 0 && <ViewerLoading />}
          {error && <div className="viewer__error" role="alert">{error}</div>}
          {navError ? (
            <div className="viewer__nav-error">
              <p>{navError}</p>
              <p className="viewer__nav-error-hint">
                j/k — try again · Esc — close viewer
              </p>
            </div>
          ) : (
            <DirectFileViewer path={curPath} filename={curFilename} videoRef={videoRef} />
          )}
        </div>
        <div className="viewer__status">
          <span className="viewer__status-nav">j/k prev/next file · M maximize · Esc close</span>
        </div>
      </div>
    );
  }

  return (
    <div
      className={`viewer${dosMode && effectiveMode === "text" && !markdownMode ? " viewer--dos" : ""}${markdownMode && effectiveMode === "text" ? " viewer--markdown" : ""}`}
      data-color-scheme={colorScheme.id}
      style={{ colorScheme: colorScheme.mode }}
    >
      <div className="viewer__header">
        <span className="viewer__filename">{curFilename}</span>
        <div className="viewer__mode-btns">
          <button
            className={`viewer__mode-btn${effectiveMode === "text" && !markdownMode ? " viewer__mode-btn--active" : ""}`}
            onClick={() => { setMarkdownMode(false); switchMode("text"); }}
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
              {isMdExt(curFilename) && (
                <button
                  className={`viewer__mode-btn${markdownMode ? " viewer__mode-btn--active" : ""}`}
                  onClick={() => setMarkdownMode(true)}
                  title="Markdown preview (4)"
                >
                  4 MD
                </button>
              )}
              {!markdownMode && (
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
            </>
          )}
          <button
            className="viewer__mode-btn"
            onClick={toggleMaximize}
            title="Toggle maximize (M)"
            aria-label="Toggle maximize"
          >
            <IconArrowsMaximize size={14} aria-hidden /> M Max
          </button>
        </div>
      </div>

      <div className="viewer__body" ref={bodyRef}>
        {pendingNavigation > 0 && <ViewerLoading />}
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
          <ImageViewer buffer={buffer} fileSize={fileSize} filename={curFilename} imageLimit={effectiveImageLimit} />
        )}
        {!error && buffer && !isImageMode && (
          <>
            {effectiveMode === "text" && markdownMode ? (
              isSmall === true ? (
                <>
                  <div
                    key="markdown-preview"
                    className="viewer__markdown"
                    ref={contentRef as React.RefObject<HTMLDivElement>}
                  />
                  {expandedMermaid && (
                    <MermaidOverlay
                      ref={mermaidOverlayRef}
                      svg={expandedMermaid}
                      onClose={() => setExpandedMermaid(null)}
                    />
                  )}
                </>
              ) : (
                <div className="viewer__markdown-unavailable">
                  Markdown preview not available for files &gt; 1 MiB
                </div>
              )
            ) : effectiveMode === "hex" ? (
              <pre
                key="hex"
                className={`viewer__pre${isSmall === false ? " viewer__pre--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLPreElement>}
              >
                {content}
              </pre>
            ) : wordWrap ? (
              <div
                key="text-wrap"
                className={`viewer__text viewer__text--wrap${isSmall === false ? " viewer__text--virtual" : ""}`}
                ref={contentRef as React.RefObject<HTMLDivElement>}
              >
                {textView.rows.map((row, i) => (
                  <div className="viewer__wrap-row" key={i} data-ln={row.gutter}>
                    <span className="viewer__content-cell">
                      {(findOpen && findQuery)
                        ? highlightLine(row.content, findMatches, visStart + i, renderedFindIdx, findLineFirstIdx.get(visStart + i) ?? 0)
                        : (row.content || " ")}
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <div
                key="text"
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
                          {highlightLine(line, findMatches, visStart + i, renderedFindIdx, findLineFirstIdx.get(visStart + i) ?? 0)}
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

      {findOpen && effectiveMode === "text" && !markdownMode && (
        <div className="viewer__find-bar">
          <input
            ref={findHistory.inputRef as React.RefObject<HTMLInputElement>}
            type="text"
            className="viewer__find-input"
            placeholder="Find…"
            value={findQuery}
            onChange={e => { setFindQuery(e.target.value); setFindIdx(0); }}
            onKeyDown={findHistory.onKeyDown}
            spellCheck={false}
          />
          {findHistory.isOpen && (
            <InputHistoryDropdown
              anchorEl={findHistory.inputRef.current}
              items={findHistory.items}
              activeIndex={findHistory.activeIndex}
              onSelect={findHistory.selectItem}
              onClose={findHistory.close}
            />
          )}
          <span className="viewer__find-count">{findLabel}</span>
          <button className="viewer__find-nav" onClick={findPrev} title="Previous (Shift+Enter)">↑</button>
          <button className="viewer__find-nav" onClick={findNext} title="Next (Enter)">↓</button>
          <button className="viewer__find-close" onClick={() => setFindOpen(false)} title="Close (Esc)">×</button>
          {isSmall === false && largeFindTruncated && (
            <span className="viewer__find-scope">first 10,000 matches</span>
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
            const canEdit = curIsLocal && !isArchiveSourceId(curSourceId) && !isRemoteSourceId(curSourceId);
            const editHint = canEdit ? " · e/F4 edit" : "";
            const markdownHint = isMdExt(curFilename) ? " · 4 md" : "";
            if (expandedMermaid) return "wheel/+/- zoom · drag/arrows pan · 0 fit · M maximize · Esc back";
            if (isImageMode) return `z zoom · j/k prev/next file · 1 text · 2 image · 3 hex${markdownHint} · M maximize${editHint}`;
            if (effectiveMode === "text") return `↑↓ PgDn/PgUp scroll · j/k prev/next · 1 text · 2 image · 3 hex${markdownHint} · M maximize · s dos · w wrap · f find${editHint}`;
            return `↑↓ PgDn/PgUp scroll · j/k prev/next file · 1 text · 2 image · 3 hex${markdownHint} · M maximize${editHint}`;
          })()}
        </span>
      </div>
    </div>
  );
}
