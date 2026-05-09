import { useState } from "react";
import type { VFSEntry } from "../ws/types";
import { useAppState } from "../store";

export interface EntryThumbProps {
  entry: VFSEntry;
  /** Real OS path used to fetch a thumbnail. When null/undefined, only the icon is rendered. */
  hostPath: string | null;
  /** Requested thumbnail size (px, longer edge). */
  size: number;
}

export function EntryThumb({ entry, hostPath, size }: EntryThumbProps) {
  const { fileServerPort } = useAppState();
  const [iconFailed, setIconFailed] = useState(false);
  const [thumbLoaded, setThumbLoaded] = useState(false);
  const [thumbFailed, setThumbFailed] = useState(false);

  const iconName = entry.iconName;
  const isThumbCandidate =
    entry.kind === "file" &&
    !!hostPath &&
    fileServerPort > 0 &&
    !thumbFailed;

  const thumbUrl = isThumbCandidate
    ? `http://127.0.0.1:${fileServerPort}/__thumb__/${size}/${encodeURIComponent(hostPath!)}`
    : null;

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
      {thumbUrl && (
        <img
          className={`panel__thumb-img${thumbLoaded ? " panel__thumb-img--loaded" : ""}`}
          src={thumbUrl}
          alt=""
          draggable={false}
          onLoad={() => setThumbLoaded(true)}
          onError={() => setThumbFailed(true)}
        />
      )}
    </div>
  );
}
