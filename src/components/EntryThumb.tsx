import { useEffect, useRef, useState } from "react";
import type { VFSEntry } from "../ws/types";
import { useAppState } from "../store";
import { IconArrowUpRight } from "@tabler/icons-react";

export interface EntryThumbProps {
  entry: VFSEntry;
  /** Fully resolved thumbnail URL. When null/undefined, only the icon is rendered. */
  thumbUrl: string | null;
}

export function EntryThumb({ entry, thumbUrl }: EntryThumbProps) {
  const { fileServerPort } = useAppState();
  const [iconFailed, setIconFailed] = useState(false);
  const [thumbLoaded, setThumbLoaded] = useState(false);
  const [thumbFailed, setThumbFailed] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const [inViewport, setInViewport] = useState(false);
  const [imageUrl, setImageUrl] = useState<string | null>(null);

  const iconName = entry.iconName;
  const isThumbCandidate =
    (entry.kind === "file" || (entry.kind === "symlink" && entry.targetKind === "file")) &&
    !!thumbUrl;
  const resolvedThumbUrl = isThumbCandidate ? thumbUrl : null;

  useEffect(() => {
    const root = rootRef.current;
    if (!root || !resolvedThumbUrl) return;
    const observer = new IntersectionObserver(([entry]) => setInViewport(entry.isIntersecting));
    observer.observe(root);
    return () => observer.disconnect();
  }, [resolvedThumbUrl]);

  useEffect(() => {
    setImageUrl(null);
    setThumbLoaded(false);
    setThumbFailed(false);
    if (!inViewport || !resolvedThumbUrl) return;
    const controller = new AbortController();
    let objectUrl: string | null = null;
    void fetch(resolvedThumbUrl, { signal: controller.signal })
      .then(response => {
        if (!response.ok || response.status === 204) throw new Error("Thumbnail unavailable");
        return response.blob();
      })
      .then(blob => {
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(blob);
        setImageUrl(objectUrl);
      })
      .catch(() => { if (!controller.signal.aborted) setThumbFailed(true); });
    return () => {
      // Release the browser connection as soon as a card leaves the viewport.
      // Backend work already queued can warm the cache after its cooldown.
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [inViewport, resolvedThumbUrl]);

  const showThumbnail = inViewport && imageUrl !== null && !thumbFailed;

  return (
    <div className="panel__thumb" ref={rootRef}>
      <div className="panel__thumb-icon" aria-hidden={showThumbnail && thumbLoaded ? "true" : "false"}>
        {iconName && !iconFailed && fileServerPort > 0 ? (
          <img
            src={`http://127.0.0.1:${fileServerPort}/__icon__/${iconName}`}
            alt=""
            draggable={false}
            onError={() => setIconFailed(true)}
          />
        ) : (
          <span className="panel__thumb-icon-fallback">{entry.kind === "directory" ? "📁" : "📄"}</span>
        )}
      </div>
      {showThumbnail && (
        <img
          className={`panel__thumb-img${thumbLoaded ? " panel__thumb-img--loaded" : ""}`}
          decoding="async"
          src={imageUrl!}
          alt=""
          draggable={false}
          onLoad={() => setThumbLoaded(true)}
          onError={() => setThumbFailed(true)}
        />
      )}
      {entry.kind === "symlink" && <span className="panel__thumb-link-marker" aria-hidden><IconArrowUpRight size={15} /></span>}
    </div>
  );
}
