import type { Notification, NotificationsPatch, NotificationsSnapshot } from "../ws/types";

export interface NotificationState extends NotificationsSnapshot {
  toastQueue: Notification[];
}

export function createNotificationState(): NotificationState {
  return { version: -1, entries: [], toastQueue: [] };
}

export function applyNotificationSnapshot(state: NotificationState, snapshot: NotificationsSnapshot): NotificationState {
  if (snapshot.version < state.version) return state;
  return { ...state, ...snapshot };
}

export function applyNotificationPatch(state: NotificationState, version: number, patch: NotificationsPatch): NotificationState {
  if (version <= state.version) return state;
  if ("clear" in patch) return { ...state, version, entries: [] };
  const removedIds = new Set(patch.removedIds);
  return {
    version,
    entries: [patch.added, ...state.entries.filter((entry) => !removedIds.has(entry.id) && entry.id !== patch.added.id)],
    toastQueue: patch.added.channels.includes("toast")
      ? [...state.toastQueue, patch.added]
      : state.toastQueue,
  };
}

export function expireNotificationToast(state: NotificationState, id: string): NotificationState {
  if (state.toastQueue[0]?.id !== id) return state;
  return { ...state, toastQueue: state.toastQueue.slice(1) };
}
