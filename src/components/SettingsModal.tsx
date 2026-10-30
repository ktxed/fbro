import { useRef, type ReactNode } from "react";
import { IconSettings, IconDeviceDesktop, IconRobot, IconShare, IconKeyboard } from "@tabler/icons-react";
import { useAppDispatch, useAppState } from "../store";
import type { SettingsTab } from "../store";
import { SettingsKeyboardTab } from "./SettingsKeyboardTab";
import { SettingsGeneralTab } from "./SettingsGeneralTab";
import { SettingsDisplayTab } from "./SettingsDisplayTab";
import { SettingsBrosTab } from "./SettingsBrosTab";
import { SettingsSharingTab } from "./SettingsSharingTab";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { rovingIndexForKey } from "./modalKeyboard";

const TABS: { id: SettingsTab; label: string; icon: ReactNode }[] = [
  { id: "general", label: "General", icon: <IconSettings size={16} aria-hidden="true" /> },
  { id: "display", label: "Display", icon: <IconDeviceDesktop size={16} aria-hidden="true" /> },
  { id: "bros", label: "Bros", icon: <IconRobot size={16} aria-hidden="true" /> },
  { id: "sharing", label: "Sharing", icon: <IconShare size={16} aria-hidden="true" /> },
  { id: "keyboard", label: "Shortcuts", icon: <IconKeyboard size={16} aria-hidden="true" /> },
];

interface SettingsModalProps {
  client: WsClient;
}

export function SettingsModal({ client }: SettingsModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const activeTab = state.pendingSettings?.activeTab ?? "general";
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const unansweredPairing = state.pendingPairing?.localDecision === null;

  const rejectUnansweredPairing = () => {
    const pairing = state.pendingPairing;
    if (!pairing || pairing.localDecision !== null) return;
    client.sendIntent("ConfirmPairing", undefined, {
      handshakeId: pairing.handshakeId,
      accept: false,
    });
    dispatch({ type: "SET_PAIRING_DECISION", handshakeId: pairing.handshakeId, accept: false });
  };

  const close = () => {
    rejectUnansweredPairing();
    dispatch({ type: "CLOSE_SETTINGS" });
  };

  const openTab = (tab: SettingsTab) => {
    if (tab !== "sharing") rejectUnansweredPairing();
    dispatch({ type: "OPEN_SETTINGS", tab });
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  return (
    <ModalLayer
      modalId="settings"
      focusKey={`pairing:${state.pendingPairing?.handshakeId ?? "none"}:${unansweredPairing}`}
      className="modal-overlay--no-click"
      onKeyDown={handleKeyDown}
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
          <div className="settings-modal__sidebar" role="tablist" aria-label="Settings tabs" aria-orientation="vertical">
            {TABS.map((tab, i) => (
              <button
                key={tab.id}
                ref={(node) => { tabRefs.current[i] = node; }}
                id={`settings-tab-${tab.id}`}
                role="tab"
                aria-selected={activeTab === tab.id}
                aria-controls={`settings-panel-${tab.id}`}
                tabIndex={activeTab === tab.id ? 0 : -1}
                data-testid={`settings-tab-${tab.id}`}
                data-modal-initial-focus={activeTab === tab.id && !unansweredPairing || undefined}
                className={`settings-modal__tab${activeTab === tab.id ? " settings-modal__tab--active" : ""}`}
                onClick={() => openTab(tab.id)}
                onKeyDown={(e) => {
                  const idx = TABS.findIndex((t) => t.id === tab.id);
                  const nextIndex = rovingIndexForKey(e.key, idx, TABS.length, "vertical");
                  if (nextIndex === null) return;
                  e.preventDefault();
                  openTab(TABS[nextIndex].id);
                  tabRefs.current[nextIndex]?.focus();
                }}
              >
                {tab.icon}
                <span>{tab.label}</span>
              </button>
            ))}
          </div>
          <div
            id={`settings-panel-${activeTab}`}
            className="settings-modal__content"
            role="tabpanel"
            aria-labelledby={`settings-tab-${activeTab}`}
          >
            {activeTab === "general" && (
              <SettingsGeneralTab general={state.settings.general} client={client} />
            )}
            {activeTab === "display" && (
              <SettingsDisplayTab general={state.settings.general} client={client} />
            )}
            {activeTab === "keyboard" && <SettingsKeyboardTab client={client} />}
            {activeTab === "bros" && <SettingsBrosTab client={client} />}
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
