import { useEffect, useRef } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { SettingsTab } from "../store";
import { SettingsGeneralTab } from "./SettingsGeneralTab";
import { SettingsDisplayTab } from "./SettingsDisplayTab";
import { SettingsSharingTab } from "./SettingsSharingTab";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";

const TABS: { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "display", label: "Display" },
  { id: "sharing", label: "Sharing" },
];

interface SettingsModalProps {
  client: WsClient;
}

export function SettingsModal({ client }: SettingsModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const activeTab = state.pendingSettings?.activeTab ?? "general";
  const firstTabRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    firstTabRef.current?.focus();
  }, []);

  const close = () => dispatch({ type: "CLOSE_SETTINGS" });

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  return (
    <ModalLayer
      modalId="settings"
      className="modal-overlay--no-click"
      onKeyDown={handleKeyDown}
      // Trap focus: prevent Tab from escaping
    >
      <ResizableModal
        modalId="settings"
        client={client}
        className="modal--settings"
        ariaModal
        ariaLabel="Settings"
        defaultSize={{ width: 820, height: 560 }}
        minSize={{ width: 620, height: 420 }}
      >
        <div className="modal__header">Settings</div>
        <div className="settings-modal__layout">
          <div className="settings-modal__sidebar" role="tablist" aria-label="Settings tabs">
            {TABS.map((tab, i) => (
              <button
                key={tab.id}
                ref={i === 0 ? firstTabRef : undefined}
                role="tab"
                aria-selected={activeTab === tab.id}
                data-testid={`settings-tab-${tab.id}`}
                className={`settings-modal__tab${activeTab === tab.id ? " settings-modal__tab--active" : ""}`}
                onClick={() => dispatch({ type: "OPEN_SETTINGS", tab: tab.id })}
                onKeyDown={(e) => {
                  const idx = TABS.findIndex((t) => t.id === tab.id);
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    const next = TABS[idx + 1];
                    if (next) dispatch({ type: "OPEN_SETTINGS", tab: next.id });
                  } else if (e.key === "ArrowUp") {
                    e.preventDefault();
                    const prev = TABS[idx - 1];
                    if (prev) dispatch({ type: "OPEN_SETTINGS", tab: prev.id });
                  }
                }}
              >
                {tab.label}
              </button>
            ))}
          </div>
          <div className="settings-modal__content" role="tabpanel">
            {activeTab === "general" && (
              <SettingsGeneralTab general={state.settings.general} client={client} />
            )}
            {activeTab === "display" && (
              <SettingsDisplayTab general={state.settings.general} client={client} />
            )}
            {activeTab === "sharing" && <SettingsSharingTab client={client} />}
          </div>
        </div>
        <div className="modal__actions">
          <button className="modal__btn" onClick={close}>
            Close
          </button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
