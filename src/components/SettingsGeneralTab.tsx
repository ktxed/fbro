import { useEffect, useRef, useState } from "react";
import type { GeneralSettings } from "../ws/types";
import type { WsClient } from "../ws/client";

interface SettingsGeneralTabProps {
  general: GeneralSettings;
  client: WsClient;
}

export function SettingsGeneralTab({ general, client }: SettingsGeneralTabProps) {
  const send = (key: string, value: boolean | number) => {
    client.sendIntent("SetGeneralOption", undefined, { key, value });
  };

  const [maxImageDraft, setMaxImageDraft] = useState(general.maxImageSizeMb);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setMaxImageDraft(general.maxImageSizeMb);
  }, [general.maxImageSizeMb]);

  const onMaxImageChange = (raw: string) => {
    const n = parseInt(raw, 10);
    if (Number.isNaN(n)) return;
    const clamped = Math.max(1, Math.min(2048, n));
    setMaxImageDraft(clamped);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => send("maxImageSizeMb", clamped), 250);
  };

  return (
    <div className="settings-tab">
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Files</h3>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.showHiddenDefault}
            onChange={(e) => send("showHiddenDefault", e.target.checked)}
          />
          <span>Show hidden files by default</span>
        </label>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.rememberWindowState}
            onChange={(e) => send("rememberWindowState", e.target.checked)}
          />
          <span>Remember window position and size</span>
        </label>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.savePanelViewMode}
            onChange={(e) => send("savePanelViewMode", e.target.checked)}
          />
          <span>Save panel listing mode (list / grid) per panel across sessions</span>
        </label>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.persistPanelTabs}
            onChange={(e) => send("persistPanelTabs", e.target.checked)}
          />
          <span>Restore all panel tabs across sessions (otherwise only the active tab returns)</span>
        </label>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">File Operations</h3>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.preserveAttributes}
            onChange={(e) => send("preserveAttributes", e.target.checked)}
          />
          <span>Preserve file timestamps and permissions when copying</span>
        </label>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Data files</h3>
        <div className="settings-tab__path-row">
          <span className="settings-tab__path-label">Session</span>
          <code className="settings-tab__path-value">{general.sessionPath}</code>
        </div>
        <div className="settings-tab__path-row">
          <span className="settings-tab__path-label">Bookmarks</span>
          <code className="settings-tab__path-value">{general.bookmarksPath}</code>
        </div>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Viewer</h3>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.ludicrousMode}
            onChange={(e) => send("ludicrousMode", e.target.checked)}
          />
          <span>Ludicrous mode (load very large images in the viewer)</span>
        </label>
        <label className="settings-tab__option" style={{ opacity: general.ludicrousMode ? 1 : 0.5 }}>
          <span style={{ minWidth: 180 }}>Max image size (MiB):</span>
          <input
            type="number"
            min={1}
            max={2048}
            step={1}
            value={maxImageDraft}
            disabled={!general.ludicrousMode}
            onChange={(e) => onMaxImageChange(e.target.value)}
            style={{ width: 90 }}
          />
        </label>
      </div>
    </div>
  );
}
