import { useState, useEffect } from "react";
import { mimeForFile } from "./media";
import { ZoomableImage } from "./ZoomableImage";

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

  useEffect(() => {
    if (!buffer) { setUrl(null); return; }
    if (fileSize !== null && fileSize > IMAGE_LIMIT) { setUrl(null); return; }
    const mime = mimeForFile(filename);
    const blob = new Blob([buffer.slice().buffer as ArrayBuffer], { type: mime });
    const u = URL.createObjectURL(blob);
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [buffer, fileSize, filename]);

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

  return <ZoomableImage src={url} alt={filename} />;
}
