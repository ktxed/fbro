import { useState, useEffect } from "react";
import { mimeForFile, isHeicExt } from "./media";
import { isMac } from "../utils/platform";
import { HeicImage } from "./HeicImage";
import { ZoomableImage } from "./ZoomableImage";
import { ViewerLoading } from "./ViewerLoading";

export const IMAGE_LIMIT = 5 * 1024 * 1024; // 5 MiB — baseline when ludicrous mode is off

interface ImageViewerProps {
  buffer: Uint8Array | null;
  fileSize: number | null;
  filename: string;
  imageLimit?: number;
  enableHeic2any?: boolean;
}

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

export function ImageViewer({ buffer, fileSize, filename, imageLimit, enableHeic2any = false }: ImageViewerProps) {
  const limit = imageLimit ?? IMAGE_LIMIT;
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!isMac && isHeicExt(filename)) { setUrl(null); return; }
    if (!buffer) { setUrl(null); return; }
    if (fileSize !== null && fileSize > limit) { setUrl(null); return; }
    const mime = mimeForFile(filename);
    const blob = new Blob([buffer.slice().buffer as ArrayBuffer], { type: mime });
    const u = URL.createObjectURL(blob);
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [buffer, fileSize, filename, limit]);

  if (fileSize !== null && fileSize > limit) {
    return (
      <div className="viewer__image-container">
        <div className="viewer__image-notice">
          Image too large to preview ({formatSize(fileSize)}; cap {formatSize(limit)})
        </div>
      </div>
    );
  }

  if (!isMac && isHeicExt(filename)) {
    if (!enableHeic2any) return <div className="viewer__image-container">
      <div className="viewer__image-notice">Enable HEIC/HEIF preview in Settings to view this image.</div>
    </div>;
    return <HeicImage filename={filename} buffer={buffer} maxBytes={limit} />;
  }
  if (!url) {
    return (
      <div className="viewer__image-container">
        <ViewerLoading />
      </div>
    );
  }

  return <ZoomableImage src={url} alt={filename} />;
}
