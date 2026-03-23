import { useState, useEffect, useCallback } from "react";
import { mimeForFile } from "./media";

export const IMAGE_LIMIT = 5 * 1024 * 1024; // 5 MiB

interface ImageViewerProps {
  buffer: Uint8Array | null;
  fileSize: number | null;
  filename: string;
}

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

export function ImageViewer({ buffer, fileSize, filename }: ImageViewerProps) {
  const [url, setUrl] = useState<string | null>(null);
  const [zoom, setZoom] = useState(false);

  // Build blob URL when buffer is available
  useEffect(() => {
    if (!buffer) { setUrl(null); return; }
    if (fileSize !== null && fileSize > IMAGE_LIMIT) { setUrl(null); return; }
    const mime = mimeForFile(filename);
    const blob = new Blob([buffer.slice().buffer as ArrayBuffer], { type: mime });
    const u = URL.createObjectURL(blob);
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [buffer, fileSize, filename]);

  // Keyboard: z toggles zoom
  const handleKey = useCallback((e: KeyboardEvent) => {
    if (e.key === "z" || e.key === "Z") setZoom(z => !z);
  }, []);

  useEffect(() => {
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [handleKey]);

  if (fileSize !== null && fileSize > IMAGE_LIMIT) {
    return (
      <div className="viewer__image-container">
        <div className="viewer__image-notice">
          Image too large to preview ({formatSize(fileSize)})
        </div>
      </div>
    );
  }

  if (!url) {
    return (
      <div className="viewer__image-container">
        <div className="viewer__loading">Loading image…</div>
      </div>
    );
  }

  return (
    <div className="viewer__image-container">
      <img
        className={`viewer__image${zoom ? " viewer__image--native" : ""}`}
        src={url}
        alt={filename}
        onClick={() => setZoom(z => !z)}
        draggable={false}
      />
    </div>
  );
}
