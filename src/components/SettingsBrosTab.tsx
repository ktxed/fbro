import { useCallback, useEffect } from "react";

import { useAppDispatch, useAppState } from "../store";
import type { WsClient } from "../ws/client";

export function SettingsBrosTab({ client }: { client: WsClient }) {
  const dispatch = useAppDispatch();
  const catalog = useAppState().broCatalog;

  const loadCatalog = useCallback(() => {
    const requestId = globalThis.crypto?.randomUUID?.()
      ?? `bro-catalog-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    dispatch({ type: "REQUEST_BRO_CATALOG", requestId });
    client.sendIntent("ListBroCatalog", undefined, { requestId });
  }, [client, dispatch]);

  useEffect(() => {
    loadCatalog();
  }, [loadCatalog]);

  return (
    <div className="settings-tab settings-bros" data-testid="settings-bros-panel">
      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Available bros</h3>
        <p className="settings-tab__hint">
          Bros process files, folders, or other entries selected in fbro.
        </p>
        {catalog.status === "loading" && (
          <p className="settings-bros__status" role="status">Looking up bros...</p>
        )}
        {catalog.status === "error" && (
          <div className="settings-bros__status" role="alert">
            <span>{catalog.error ?? "Failed to load bros."}</span>
            <button className="modal__btn" type="button" onClick={loadCatalog}>Retry</button>
          </div>
        )}
        {catalog.status === "ready" && catalog.bros.length === 0 && (
          <p className="settings-bros__status">No bros are available on this system.</p>
        )}
        {catalog.status === "ready" && catalog.bros.length > 0 && (
          <ul className="settings-bros__list" aria-label="Available bros">
            {catalog.bros.map((bro) => (
              <li className="settings-bros__item" key={bro.id} data-testid="settings-bro-item">
                <strong>{bro.name}</strong>
                <span>{bro.description}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
