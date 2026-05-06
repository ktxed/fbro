import type React from "react";
import { isMac } from "../utils/platform";
import { isImageExt, isVideoExt, isPdfExt } from "./media";
import { ZoomableImage } from "./ZoomableImage";

export const IMAGE_DIRECT_LIMIT = 10 * 1024 * 1024; // 10 MiB — baseline when ludicrous mode is off

// macOS WebKit's <video> uses AVFoundation, which has no Matroska demuxer.
// Skipping direct mode lets the viewer fall through to its hex preview.
const MAC_UNSUPPORTED_VIDEO_EXTS = new Set(["mkv"]);

function isPlayableVideo(filename: string): boolean {
  if (!isVideoExt(filename)) return false;
  if (!isMac) return true;
  const ext = filename.split(".").pop()?.toLowerCase() ?? "";
  return !MAC_UNSUPPORTED_VIDEO_EXTS.has(ext);
}

export function canUseDirect(
  isLocal: boolean,
  filename: string,
  fileSize: number | null,
  imageLimit: number = IMAGE_DIRECT_LIMIT,
): boolean {
  if (!isLocal) return false;
  if (isPdfExt(filename) || isPlayableVideo(filename)) return true;
  if (isImageExt(filename)) return fileSize !== null && fileSize < imageLimit;
  return false;
}

interface DirectFileViewerProps {
  path: string;
  filename: string;
  /** Override the file server port (defaults to reading from URL query params). */
  fsPort?: number;
  /** Forwarded to the underlying <video> element when the file is a video. */
  videoRef?: React.Ref<HTMLVideoElement>;
}

export function DirectFileViewer({ path, filename, fsPort: fsPropPort, videoRef }: DirectFileViewerProps) {
  // Serve all local files through the HTTP file server.
  // On Windows, paths don't start with / (e.g. C:/...) so we add one for valid URLs.
  // (localfile:// was used for images on Linux/macOS to avoid GStreamer intercepting
  // <video> elements, but GStreamer does not affect <img>, and localfile:// is not
  // supported by WebView2 on Windows without additional setup.)
  const fsPort = fsPropPort ?? Number(new URLSearchParams(window.location.search).get("fsport") ?? "0");
  const urlPath = path.startsWith("/") ? path : `/${path}`;
  const url = `http://127.0.0.1:${fsPort}${urlPath}`;

  if (isPdfExt(filename)) {
    return <embed className="viewer__embed" src={url} type="application/pdf" />;
  }
  if (isVideoExt(filename)) {
    return (
      <div className="viewer__video-container">
        <video ref={videoRef} className="viewer__video" src={url} controls />
      </div>
    );
  }
  // Image
  return <ZoomableImage src={url} alt={filename} />;
}
