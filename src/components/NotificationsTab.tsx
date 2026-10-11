import { useEffect, useRef, useState } from "react";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";
import { rovingIndexForKey } from "./modalKeyboard";

const timestampFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "medium" });

export function NotificationsTab({ client }: { client: WsClient }) {
  const { notifications, notificationConnectionReady, notificationPublishError } = useAppState();
  const listRef = useRef<HTMLDivElement>(null);
  const clearButtonRef = useRef<HTMLButtonElement>(null);
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const [activeId, setActiveId] = useState<string | null>(null);
  const [clearRequest, setClearRequest] = useState<string | null>(null);
  const clearRequestRef = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const entries = notifications.entries;
  const activeIndex = Math.max(0, entries.findIndex((entry) => entry.id === activeId));
  const currentId = entries[activeIndex]?.id;

  useEffect(() => {
    // A focused empty list can gain its first entry without another focus event.
    // Pin that fallback entry so subsequent arrivals preserve the browsed row.
    if (currentId && document.activeElement === listRef.current && activeId !== currentId) setActiveId(currentId);
    if (currentId) rowRefs.current.get(currentId)?.scrollIntoView({ block: "nearest" });
  }, [activeId, currentId, entries]);

  useEffect(() => client.subscribeMessages((message) => {
    if ((message.type !== "ClearNotificationsOk" && message.type !== "ClearNotificationsFailed")
      || message.requestId !== clearRequestRef.current) return;
    clearRequestRef.current = null;
    setClearRequest(null);
    setError(message.type === "ClearNotificationsFailed" ? message.message : null);
  }), [client]);

  useEffect(() => {
    if (notificationConnectionReady || !clearRequestRef.current) return;
    clearRequestRef.current = null;
    setClearRequest(null);
    setError("Connection lost. Check notification history after reconnecting, then retry if needed.");
  }, [notificationConnectionReady]);

  const clearHistory = () => {
    if (clearRequestRef.current || !notificationConnectionReady || entries.length === 0) return;
    // Clear becomes disabled while submitting and when empty. Hand its focus
    // to the persistent list before disabling it, preserving other owners.
    if (document.activeElement === clearButtonRef.current) listRef.current?.focus();
    const requestId = crypto.randomUUID();
    clearRequestRef.current = requestId;
    setClearRequest(requestId);
    setError(null);
    client.sendIntent("ClearNotifications", undefined, { requestId });
  };

  return (
    <div className="notification-history" data-testid="notification-history">
      <div className="notification-history__toolbar">
        <span className="notification-history__count">{entries.length} / 1,000 retained this session</span>
        <button ref={clearButtonRef} className="modal__btn" type="button" data-testid="notifications-clear"
          disabled={!!clearRequest || !notificationConnectionReady || entries.length === 0} onClick={clearHistory}>
          {clearRequest ? "Clearing…" : "Clear history"}
        </button>
      </div>
      {(error || notificationPublishError) && <div className="notification-history__error" role="alert">{error || notificationPublishError}</div>}
      <div className="notification-history__list" ref={listRef} role="listbox" tabIndex={0}
        aria-label="Notification history" aria-activedescendant={currentId ? `notification-${currentId}` : undefined}
        data-testid="notifications-list"
        onFocus={() => { if (currentId && activeId !== currentId) setActiveId(currentId); }}
        onKeyDown={(event) => {
          if (event.defaultPrevented || entries.length === 0) return;
          const next = rovingIndexForKey(event.key, activeIndex, entries.length, "vertical");
          if (next === null) return;
          event.preventDefault();
          setActiveId(entries[next].id);
        }}>
        {entries.length === 0 ? <div className="jobs-modal__empty">No notifications.</div> : entries.map((entry, index) => (
          <div key={entry.id} id={`notification-${entry.id}`} role="option" aria-selected={index === activeIndex}
            data-testid="notification-entry" data-notification-id={entry.id} data-level={entry.level}
            ref={(node) => { if (node) rowRefs.current.set(entry.id, node); else rowRefs.current.delete(entry.id); }}
            className={`notification-history__row${index === activeIndex ? " notification-history__row--active" : ""}`}
            onClick={() => { listRef.current?.focus(); setActiveId(entry.id); }}>
            <time className="notification-history__time" dateTime={new Date(entry.timestamp).toISOString()}>
              {timestampFormat.format(entry.timestamp)}
            </time>
            <span className={`notification-history__level notification-history__level--${entry.level}`}>{entry.level}</span>
            <span className="notification-history__message">{entry.message}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
