import { isImageExt, isVideoExt, isPdfExt } from "./media";

export const IMAGE_DIRECT_LIMIT = 10 * 1024 * 1024; // 10 MiB

export function canUseDirect(isLocal: boolean, filename: string, fileSize: number | null): boolean {
  if (!isLocal) return false;
  if (isPdfExt(filename) || isVideoExt(filename)) return true;
  if (isImageExt(filename)) return fileSize !== null && fileSize < IMAGE_DIRECT_LIMIT;
  return false;
}

interface DirectFileViewerProps {
  path: string;
  filename: string;
}

export function DirectFileViewer({ path, filename }: DirectFileViewerProps) {
  // For video/PDF: use the local HTTP file server so GStreamer can stream via http://.
  // For images: use the localfile:// custom scheme (GStreamer is not involved).
  const fsPort = new URLSearchParams(window.location.search).get("fsport") ?? "0";
  const url = (isVideoExt(filename) || isPdfExt(filename))
    ? `http://127.0.0.1:${fsPort}${path}`
    : `localfile://localhost${path}`;

  if (isPdfExt(filename)) {
    return <embed className="viewer__embed" src={url} type="application/pdf" />;
  }
  if (isVideoExt(filename)) {
    return (
      <div className="viewer__video-container">
        <video className="viewer__video" src={url} controls />
      </div>
    );
  }
  // Image
  return (
    <div className="viewer__image-container">
      <img className="viewer__image" src={url} alt={filename} draggable={false} />
    </div>
  );
}
