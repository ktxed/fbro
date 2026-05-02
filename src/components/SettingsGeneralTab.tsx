import type { GeneralSettings } from "../ws/types";
import type { WsClient } from "../ws/client";

interface SettingsGeneralTabProps {
  general: GeneralSettings;
  client: WsClient;
}

export function SettingsGeneralTab({ general, client }: SettingsGeneralTabProps) {
  const send = (key: string, value: boolean) => {
    client.sendIntent("SetGeneralOption", undefined, { key, value });
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
    </div>
  );
}
