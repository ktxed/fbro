// ─── Multimedia provider ─────────────────────────────────────────────────────
// Maps file extensions to renderer kinds and MIME types.
// Future media types (audio, video, PDF) add entries here and a render branch.

export type RendererKind = "text" | "image" | "hex";

const IMAGE_EXTENSIONS: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg",
  gif: "image/gif", webp: "image/webp", svg: "image/svg+xml",
  bmp: "image/bmp", ico: "image/x-icon", avif: "image/avif",
};

function extOf(filename: string): string {
  return filename.split(".").pop()?.toLowerCase() ?? "";
}

export function isImageExt(filename: string): boolean {
  return extOf(filename) in IMAGE_EXTENSIONS;
}

export function mimeForFile(filename: string): string {
  return IMAGE_EXTENSIONS[extOf(filename)] ?? "application/octet-stream";
}

export function detectRenderer(filename: string, isBinary: boolean): RendererKind {
  if (isImageExt(filename)) return "image";
  return isBinary ? "hex" : "text";
}
