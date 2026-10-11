import { useCallback, useEffect, useRef, type Dispatch } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { NotificationInput } from "../ws/types";
import type { WsClient } from "../ws/client";

export interface NotificationRequest {
  requestId: string;
  input: NotificationInput;
}

export type NotificationRequestAction = { type: "REQUEST_NOTIFICATION"; request: NotificationRequest };

/** Publish feedback through the backend; history and delivery arrive as patches. */
export function publishNotification(dispatch: Dispatch<NotificationRequestAction>, input: NotificationInput) {
  dispatch({ type: "REQUEST_NOTIFICATION", request: { requestId: crypto.randomUUID(), input } });
}

export function useNotifications() {
  const dispatch = useAppDispatch();
  return useCallback((input: NotificationInput) => publishNotification(dispatch, input), [dispatch]);
}

/** Reducers enqueue requests without side effects; this bridge submits them once
 * per connection and keeps unacknowledged requests available after reconnect. */
export function useNotificationBridge(client: WsClient | null) {
  const { notificationRequests, notificationConnectionReady, sessionId } = useAppState();
  const submitted = useRef(new Set<string>());
  const connection = useRef<string | null>(null);
  useEffect(() => {
    if (!client || !notificationConnectionReady) return;
    if (connection.current !== sessionId) {
      connection.current = sessionId;
      submitted.current.clear();
    }
    const pendingIds = new Set(notificationRequests.map((request) => request.requestId));
    for (const id of submitted.current) {
      if (!pendingIds.has(id)) submitted.current.delete(id);
    }
    for (const request of notificationRequests) {
      if (submitted.current.has(request.requestId)) continue;
      submitted.current.add(request.requestId);
      client.sendIntent("PublishNotification", undefined, { requestId: request.requestId, ...request.input });
    }
  }, [client, notificationRequests, notificationConnectionReady, sessionId]);
}
