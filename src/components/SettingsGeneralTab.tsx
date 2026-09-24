import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { GeneralSettings } from "../ws/types";
import type { WsClient } from "../ws/client";

import { stepNumberInputOnWheel } from "../utils/numberInputWheel";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";

interface SettingsGeneralTabProps {
  general: GeneralSettings;
  client: WsClient;
}

interface ShellCommandStatus {
  command: string;
  path: string | null;
  installed: boolean;
  managed: boolean;
}

export function SettingsGeneralTab({ general, client }: SettingsGeneralTabProps) {
  const send = (key: string, value: boolean | number | string) => {
    client.sendIntent("SetGeneralOption", undefined, { key, value });
  };

  const [maxImageDraft, setMaxImageDraft] = useState(general.maxImageSizeMb);
  const [shellCommand, setShellCommand] = useState<ShellCommandStatus | null>(null);
  const [commandStatus, setCommandStatus] = useState<string | null>(null);
  const [commandBusy, setCommandBusy] = useState(false);
  const [commandProbing, setCommandProbing] = useState(true);
  const [confirmLocationReset, setConfirmLocationReset] = useState(false);
  const locationResetCancelRef = useRef<HTMLButtonElement>(null);
  const [sevenzipPathDraft, setSevenzipPathDraft] = useState(general.sevenzipCustomPath);
  const [fedupIgnoreDraft, setFedupIgnoreDraft] = useState(general.fedupDefaultIgnorePatterns);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const sevenzipPathDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fedupIgnoreDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setSevenzipPathDraft(general.sevenzipCustomPath);
  }, [general.sevenzipCustomPath]);

  useEffect(() => {
    setFedupIgnoreDraft(general.fedupDefaultIgnorePatterns);
  }, [general.fedupDefaultIgnorePatterns]);

  useEffect(() => {
    setMaxImageDraft(general.maxImageSizeMb);
  }, [general.maxImageSizeMb]);

  const probeCommand = async () => {
    setCommandProbing(true);
    try {
      const result = await invoke<ShellCommandStatus>("probe_fbro_command");
      setShellCommand(result);
    } catch (error) {
      setCommandStatus(error instanceof Error ? error.message : String(error));
      setShellCommand(null);
    } finally {
      setCommandProbing(false);
    }
  };

  useEffect(() => {
    void probeCommand();
  }, []);

  useEffect(() => () => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (sevenzipPathDebounce.current) clearTimeout(sevenzipPathDebounce.current);
    if (fedupIgnoreDebounce.current) clearTimeout(fedupIgnoreDebounce.current);
  }, []);

  const onMaxImageChange = (raw: string) => {
    const n = parseInt(raw, 10);
    if (Number.isNaN(n)) return;
    const clamped = Math.max(1, Math.min(2048, n));
    setMaxImageDraft(clamped);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => send("maxImageSizeMb", clamped), 250);
  };

  const installCommand = async () => {
    setCommandBusy(true);
    setCommandStatus(null);
    try {
      const result = await invoke<{ path: string; command: string }>("install_fbro_command");
      setCommandStatus(`Installed ${result.command} at ${result.path}`);
      await probeCommand();
    } catch (error) {
      setCommandStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setCommandBusy(false);
    }
  };

  const removeCommand = async () => {
    setCommandBusy(true);
    setCommandStatus(null);
    try {
      const result = await invoke<{ path: string; command: string }>("remove_fbro_command");
      setCommandStatus(`Removed ${result.command} from ${result.path}`);
      await probeCommand();
    } catch (error) {
      setCommandStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setCommandBusy(false);
    }
  };

  const commandHint = shellCommand?.installed
    ? shellCommand.managed
      ? `Installed at ${shellCommand.path}`
      : `Detected at ${shellCommand.path}; removal is only available for the fbro-managed command.`
    : "Not installed";
  const commandAction = shellCommand?.installed && shellCommand.managed ? "remove" : "install";
  const commandButtonLabel = commandBusy
    ? commandAction === "remove"
      ? "Removing…"
      : "Installing…"
    : commandProbing
      ? "Checking…"
      : commandAction === "remove"
        ? "Remove fbro command"
        : shellCommand?.installed
          ? "Install managed fbro command"
          : "Install fbro command";

  const resetLocationSorts = () => {
    setConfirmLocationReset(false);
    client.sendIntent("ResetLocationSortSettings");
  };

  return (
    <div className="settings-tab">
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Shell command</h3>
        <div className="settings-tab__option">
          <button
            className={`settings-button ${commandAction === "remove" ? "settings-button--danger" : "settings-button--primary"}`}
            type="button"
            onClick={commandAction === "remove" ? removeCommand : installCommand}
            disabled={commandBusy || commandProbing}
          >
            {commandButtonLabel}
          </button>
          <span>Use <code>fbro .</code> or <code>fbro ~/my_folder</code> from a terminal</span>
        </div>
        <div className="settings-tab__hint">
          {commandProbing ? "Checking fbro command…" : commandHint}
        </div>
        {commandStatus && (
          <div className="settings-tab__hint">
            {commandStatus}
          </div>
        )}
      </div>
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
            data-testid="remember-sort-per-location"
            checked={general.rememberSortPerLocation}
            onChange={(e) => send("rememberSortPerLocation", e.target.checked)}
          />
          <span>Remember sorting per location</span>
        </label>
        <div className="settings-tab__option">
          <span data-testid="location-settings-count">
            {general.locationSettingsCount === 0
              ? "No remembered locations"
              : `${general.locationSettingsCount} remembered location${general.locationSettingsCount === 1 ? "" : "s"}${general.rememberSortPerLocation ? "" : " retained for up to 30 days"}`}
          </span>
          <button
            className="settings-button settings-button--danger"
            data-testid="reset-location-sorts"
            type="button"
            disabled={general.locationSettingsCount === 0}
            onClick={() => setConfirmLocationReset(true)}
          >
            Reset remembered sorting
          </button>
        </div>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.persistPanelTabs}
            onChange={(e) => send("persistPanelTabs", e.target.checked)}
          />
          <span>Restore all panel tabs across sessions (otherwise only the active tab returns)</span>
        </label>
        <label className="settings-tab__option">
          <span>Quick terminal editor</span>
          <select value={general.quickTerminalEditor} onChange={(e) => send("quickTerminalEditor", e.target.value)}>
            <option value="nano">Nano</option>
            <option value="vim">Vim</option>
          </select>
        </label>
        <div className="settings-tab__option" style={{ flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
          <span>Fedup default ignore patterns</span>
          <div className="settings-tab__fedup-ignore-editor">
            <textarea
              className="settings-tab__fedup-ignore-input"
              value={fedupIgnoreDraft}
              onChange={(e) => {
                setFedupIgnoreDraft(e.target.value);
                if (fedupIgnoreDebounce.current) clearTimeout(fedupIgnoreDebounce.current);
                const v = e.target.value;
                fedupIgnoreDebounce.current = setTimeout(() => send("fedupDefaultIgnorePatterns", v), 350);
              }}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              rows={5}
            />
          </div>
          <div className="settings-tab__hint">
            One wildcard name pattern per line. Fedup can use these to skip files and folders before scanning.
          </div>
        </div>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Process panel</h3>
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.procTreeMode}
            onChange={(e) => send("procTreeMode", e.target.checked)}
          />
          <span>Process tree view (navigate into a process to see its children)</span>
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
        <label className="settings-tab__option">
          <input
            type="checkbox"
            checked={general.autoResumeCopies}
            onChange={(e) => send("autoResumeCopies", e.target.checked)}
          />
          <span>Auto-retry interrupted copies</span>
        </label>
      </div>
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">7-Zip extractor</h3>
        <div className="settings-tab__option" style={{ flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
          <label className="settings-tab__option" style={{ margin: 0 }}>
            <input
              type="radio"
              name="sevenzip-extractor"
              checked={general.sevenzipExtractor === "auto"}
              onChange={() => send("sevenzipExtractor", "auto")}
            />
            <span>Auto-detect on PATH (recommended)</span>
          </label>
          <label className="settings-tab__option" style={{ margin: 0 }}>
            <input
              type="radio"
              name="sevenzip-extractor"
              checked={general.sevenzipExtractor === "custom"}
              onChange={() => send("sevenzipExtractor", "custom")}
            />
            <span>Custom path</span>
          </label>
          <input
            type="text"
            placeholder="C:\Program Files\7-Zip\7z.exe"
            value={sevenzipPathDraft}
            disabled={general.sevenzipExtractor !== "custom"}
            onChange={(e) => {
              setSevenzipPathDraft(e.target.value);
              if (sevenzipPathDebounce.current) clearTimeout(sevenzipPathDebounce.current);
              const v = e.target.value;
              sevenzipPathDebounce.current = setTimeout(() => send("sevenzipCustomPath", v), 350);
            }}
            style={{ width: "100%", maxWidth: 480, marginLeft: 24 }}
          />
          <label className="settings-tab__option" style={{ margin: 0 }}>
            <input
              type="radio"
              name="sevenzip-extractor"
              checked={general.sevenzipExtractor === "internal"}
              onChange={() => send("sevenzipExtractor", "internal")}
            />
            <span>Internal (pure Rust, no native dependency)</span>
          </label>
        </div>
        <div className="settings-tab__hint">
          {general.sevenzipResolvedPath
            ? `Using ${general.sevenzipResolvedPath}`
            : general.sevenzipExtractor === "internal"
              ? "Using the in-process extractor"
              : general.sevenzipExtractor === "custom"
                ? sevenzipPathDraft.trim() === ""
                  ? "No custom path set — will fall back to the in-process extractor"
                  : `Path not found — will fall back to the in-process extractor`
                : "No 7z / 7zz / 7za binary found on PATH — will fall back to the in-process extractor"}
        </div>
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
            onWheel={stepNumberInputOnWheel}
            style={{ width: 90 }}
          />
        </label>
      </div>
      {confirmLocationReset && (
        <ModalLayer
          modalId="reset-location-sorts"
          initialFocusRef={locationResetCancelRef}
          className="modal-overlay--no-click"
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Escape") setConfirmLocationReset(false);
          }}
        >
          <ModalDialog
            ariaLabel="Reset remembered sorting"
            role="alertdialog"
            testId="reset-location-sorts-modal"
          >
            <div className="modal__header">Reset remembered sorting</div>
            <div className="modal__body">
              <p className="modal__summary">
                Forget sorting remembered for {general.locationSettingsCount} location{general.locationSettingsCount === 1 ? "" : "s"}? This cannot be undone.
              </p>
              <div className="modal__actions">
                <button
                  className="settings-button settings-button--danger"
                  data-testid="confirm-reset-location-sorts"
                  type="button"
                  onClick={resetLocationSorts}
                >
                  Reset sorting
                </button>
                <button
                  ref={locationResetCancelRef}
                  className="modal__btn"
                  type="button"
                  onClick={() => setConfirmLocationReset(false)}
                >
                  Cancel
                </button>
              </div>
            </div>
          </ModalDialog>
        </ModalLayer>
      )}
    </div>
  );
}
