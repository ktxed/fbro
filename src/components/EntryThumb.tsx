import { useState } from "react";
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

  const iconName = entry.iconName;
  const isThumbCandidate =
    (entry.kind === "file" || (entry.kind === "symlink" && entry.targetKind === "file")) &&
    !!thumbUrl &&
    !thumbFailed;
  const resolvedThumbUrl = isThumbCandidate ? thumbUrl : null;

  return (
    <div className="panel__thumb">
      <div className="panel__thumb-icon" aria-hidden={thumbLoaded ? "true" : "false"}>
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
      {resolvedThumbUrl && (
        <img
          className={`panel__thumb-img${thumbLoaded ? " panel__thumb-img--loaded" : ""}`}
          // Defer the HTTP request until the card is actually in the
          // viewport. Without this the browser fetches a thumbnail for
          // every mounted card — including off-screen buffer rows — which
          // floods the BE worker pool with stale work during fast scrolls.
          loading="lazy"
          decoding="async"
          src={resolvedThumbUrl}
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
