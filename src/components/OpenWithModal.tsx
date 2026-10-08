import { useEffect, useMemo, useRef, useState } from "react";

import { useAppDispatch, useAppState, type PendingOpenWith } from "../store";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

interface OpenWithModalProps {
  client: WsClient;
  openWith: PendingOpenWith;
}

export function OpenWithModal({ client, openWith }: OpenWithModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);

  const apps = openWith.apps;

  useEffect(() => {
    if (apps.length === 0) {
      setSelectedIndex(0);
      return;
    }
    setSelectedIndex((current) => Math.min(current, apps.length - 1));
  }, [apps]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [apps, selectedIndex]);

  useEffect(() => {
    const panel = state.panels[openWith.panelId];
    if (!panel || panel.path.replace(/[/\\]+$/, "") !== openWith.dirPath) {
      dispatch({ type: "CLOSE_OPEN_WITH" });
      return;
    }
    const entry = panel.entries[openWith.absIndex - (panel.windowStart ?? 0)];
    if (!entry || entry.name !== openWith.name || entry.kind !== openWith.kind
      || (entry.sourcePath ?? `${openWith.dirPath}/${entry.name}`) !== openWith.fullPath) {
      dispatch({ type: "CLOSE_OPEN_WITH" });
    }
  }, [dispatch, openWith.absIndex, openWith.dirPath, openWith.fullPath, openWith.kind, openWith.name, openWith.panelId, state.panels]);

  const close = () => dispatch({ type: "CLOSE_OPEN_WITH" });

  const selectedApp = useMemo(
    () => apps[selectedIndex],
    [apps, selectedIndex],
  );

  const launchApp = (appId: string) => {
    close();
    client.sendIntent("LaunchOpenWithApp", undefined, {
      requestId: openWith.requestId,
      appId,
    });
  };

  const launchSelected = () => {
    if (!selectedApp) return;
    launchApp(selectedApp.id);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (isNativeActivationTarget(e.target)) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (apps.length > 0) {
        setSelectedIndex((current) => (current + 1) % apps.length);
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (apps.length > 0) {
        setSelectedIndex((current) => (current - 1 + apps.length) % apps.length);
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      launchSelected();
    }
  };

  return (
    <ModalLayer
      modalId="open-with"
      initialFocusRef={listRef}
      onClick={close}
      onKeyDown={onKeyDown}
    >
      <ResizableModal
        ref={dialogRef}
        modalId="open-with"
        client={client}
        className="command-palette open-with"
        ariaModal
        ariaLabel="Open With"
        testId="open-with-dialog"
        tabIndex={-1}
        defaultSize={{ width: 560, height: 420 }}
        minSize={{ width: 420, height: 300 }}
      >
        <div className="modal__header">Open With…</div>
        <div className="modal__body open-with__body">
          <div className="open-with__target">{openWith.name}</div>
          <div
            className="command-palette__list open-with__list"
            ref={listRef}
            role="listbox"
            aria-label="Applications"
            aria-activedescendant={apps[selectedIndex] ? `open-with-option-${selectedIndex}` : undefined}
            tabIndex={0}
          >
            {openWith.status === "loading" && (
              <div className="command-palette__empty">Looking up applications…</div>
            )}
            {openWith.status === "error" && (
              <div className="command-palette__empty">{openWith.error ?? "Failed to load applications."}</div>
            )}
            {openWith.status === "ready" && apps.length === 0 && (
              <div className="command-palette__empty">No applications available.</div>
            )}
            {openWith.status === "ready" && apps.map((app, index) => {
              const selected = index === selectedIndex;
              return (
                <button
                  key={app.id}
                  id={`open-with-option-${index}`}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  tabIndex={-1}
                  className={`command-palette__item${selected ? " command-palette__item--selected" : ""}`}
                  data-selected={selected ? "true" : "false"}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseEnter={() => setSelectedIndex(index)}
                  onClick={() => launchApp(app.id)}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{app.label}</span>
                    <span className="command-palette__item-category">
                      {app.detail ?? (app.source === "supplement" ? "Detected application" : "Registered application")}
                    </span>
                  </span>
                  <span className="command-palette__item-binding">
                    {app.isDefault ? "Default" : ""}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
