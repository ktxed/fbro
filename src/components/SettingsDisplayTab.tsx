import { useEffect, useRef, useState } from "react";
import type { GeneralSettings } from "../ws/types";
import type { WsClient } from "../ws/client";
import { isLinux } from "../utils/platform";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface SettingsDisplayTabProps {
  general: GeneralSettings;
  client: WsClient;
}

export function SettingsDisplayTab({ general, client }: SettingsDisplayTabProps) {
  const send = (key: string, value: boolean | number) => {
    client.sendIntent("SetGeneralOption", undefined, { key, value });
  };

  const [maxImageDraft, setMaxImageDraft] = useState(general.thumbnailMaxImageSizeMb);
  const [frameSkipDraft, setFrameSkipDraft] = useState(general.thumbnailVideoFrameSkip);
  const maxImageTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const frameSkipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setMaxImageDraft(general.thumbnailMaxImageSizeMb);
  }, [general.thumbnailMaxImageSizeMb]);

  useEffect(() => {
    setFrameSkipDraft(general.thumbnailVideoFrameSkip);
  }, [general.thumbnailVideoFrameSkip]);

  const onMaxImageChange = (raw: string) => {
    const n = parseInt(raw, 10);
    if (Number.isNaN(n)) return;
    const clamped = Math.max(1, Math.min(4096, n));
    setMaxImageDraft(clamped);
    if (maxImageTimer.current) clearTimeout(maxImageTimer.current);
    maxImageTimer.current = setTimeout(
      () => send("thumbnailMaxImageSizeMb", clamped),
      250,
    );
  };

  const onFrameSkipChange = (raw: string) => {
    const n = parseInt(raw, 10);
    if (Number.isNaN(n)) return;
    const clamped = Math.max(0, Math.min(60, n));
    setFrameSkipDraft(clamped);
    if (frameSkipTimer.current) clearTimeout(frameSkipTimer.current);
    frameSkipTimer.current = setTimeout(
      () => send("thumbnailVideoFrameSkip", clamped),
      250,
    );
  };

  const onClearCache = () => {
    client.sendIntent("ClearThumbnailCache");
  };

  if (!isLinux) {
    return (
      <div className="settings-tab">
        <h3>Thumbnails</h3>
        <p className="settings-tab__hint">
          On this platform fbro uses the OS-native thumbnail provider. The
          fbro-managed thumbnailer settings below only apply on Linux.
        </p>
      </div>
    );
  }

  return (
    <div className="settings-tab">
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Thumbnails</h3>
        <p className="settings-tab__hint">
          fbro generates thumbnails for images and videos itself on Linux,
          since freedesktop thumbnailers are not always installed.
        </p>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.enableVideoThumbnails}
            onChange={(e) => send("enableVideoThumbnails", e.target.checked)}
          />
          <span>Generate thumbnails for video files</span>
        </label>
        <label className="settings-tab__option">
          <span style={{ minWidth: 220 }}>Skip images larger than (MiB):</span>
          <input
            type="number"
            min={1}
            max={4096}
            step={1}
            value={maxImageDraft}
            onChange={(e) => onMaxImageChange(e.target.value)}
            style={{ width: 90 }}
          />
        </label>
        <label
          className="settings-tab__option"
          style={{ opacity: general.enableVideoThumbnails ? 1 : 0.5 }}
        >
          <span style={{ minWidth: 220 }}>Skip first N video frames:</span>
          <input
            type="number"
            min={0}
            max={60}
            step={1}
            value={frameSkipDraft}
            disabled={!general.enableVideoThumbnails}
            onChange={(e) => onFrameSkipChange(e.target.value)}
            style={{ width: 90 }}
          />
        </label>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Cache</h3>
        <div className="settings-tab__path-row">
          <span className="settings-tab__path-label">Directory</span>
          <code className="settings-tab__path-value">
            {general.thumbnailCacheDir || "(unavailable)"}
          </code>
        </div>
        <div className="settings-tab__path-row">
          <span className="settings-tab__path-label">Size</span>
          <span className="settings-tab__path-value">
            {general.thumbnailCacheItemCount === 0
              ? "empty"
              : `${general.thumbnailCacheItemCount} item${general.thumbnailCacheItemCount === 1 ? "" : "s"}, ${formatBytes(general.thumbnailCacheSizeBytes)}`}
          </span>
        </div>
        <div className="settings-tab__option">
          <button
            className="modal__btn"
            type="button"
            onClick={onClearCache}
            disabled={general.thumbnailCacheItemCount === 0}
          >
            Clear thumbnail cache
          </button>
        </div>
      </div>
    </div>
  );
}
