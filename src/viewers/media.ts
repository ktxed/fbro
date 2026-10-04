// ─── Multimedia provider ─────────────────────────────────────────────────────
// Maps file extensions to renderer kinds and MIME types.
import { isMac } from "../utils/platform";

export type RendererKind = "text" | "image" | "video" | "pdf" | "hex";

const IMAGE_EXTENSIONS: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg",
  gif: "image/gif", webp: "image/webp", svg: "image/svg+xml",
  bmp: "image/bmp", ico: "image/x-icon", avif: "image/avif",
  // WKWebView supports HEIC/HEIF from Safari 17. Other WebViews use hex.
  ...(isMac ? { heic: "image/heic", heif: "image/heif" } : {}),
};

const VIDEO_EXTENSIONS = new Set(["mp4", "webm", "ogv", "mkv", "mov"]);
const PDF_EXTENSIONS   = new Set(["pdf"]);

function extOf(filename: string): string {
  return filename.split(".").pop()?.toLowerCase() ?? "";
}

export function isImageExt(filename: string): boolean {
  return extOf(filename) in IMAGE_EXTENSIONS;
}

export function isVideoExt(filename: string): boolean {
  return VIDEO_EXTENSIONS.has(extOf(filename));
}

export function isPdfExt(filename: string): boolean {
  return PDF_EXTENSIONS.has(extOf(filename));
}

const MD_EXTENSIONS = new Set(["md", "markdown", "mdown"]);

export function isMdExt(filename: string): boolean {
  return MD_EXTENSIONS.has(extOf(filename));
}

export function mimeForFile(filename: string): string {
  return IMAGE_EXTENSIONS[extOf(filename)] ?? "application/octet-stream";
}

export function detectRenderer(filename: string, isBinary: boolean): "text" | "image" | "hex" {
  if (isImageExt(filename)) return "image";
  return isBinary ? "hex" : "text";
}
