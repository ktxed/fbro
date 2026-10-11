# Notifications

Notifications are user feedback with a backend-owned record and selectable
delivery channels. They replace the previous one-shot transient hint mechanism.
General diagnostic logs and inline-only dialog errors are not automatically
notifications.

## Ownership and lifetime

The running backend owns one notification service. It assigns each publication a
unique ID and a UTC timestamp in milliseconds, records it before delivery, and
retains the newest 1,000 entries in publication order. Oldest entries are evicted
when full. Repeated messages are separate publications.

History is memory-only: frontend reloads and reconnects retain it, backend exit
clears it. No user preference or history is stored in localStorage or on disk.
Notification publication and history housekeeping are excluded from the
persistent command journal, which records semantic user commands rather than
delivery chatter. Selected stderr logging remains a separate delivery channel.
Storage and delivery have separate boundaries so persistence and OS delivery can
be introduced without changing publishers.

## Publishing and channels

`NotificationInput` contains `message`, optional `level` (`info`, `warning`, or
`error`), and optional `channels` (`toast`, `log`). The defaults are `info` and
both channels. An explicit empty channel list retains history without delivery.
Log-only publications also appear in history.

Frontend command code uses `publishNotification(dispatch, input)`; components
and hooks use `useNotifications()`. These enqueue publication requests, not
optimistic history or toasts. One effect bridge submits requests after the
connection handshake and retains them until acknowledgement. Stable request IDs
prevent duplicate React effect publication and are reused after reconnecting;
the backend deduplicates while the original entry remains retained.

Backend feedback is published at the originating operation or worker boundary.
Existing operation replies, including `*ClipboardWritten`, remain available to
their consumers but do not create another frontend notification. Conditional
inline-error versus toast fallback remains a pure frontend decision and submits
through the same service. No WebSocket or logging side effects run in reducers.

Confirmations use `info`; unavailable operations, recovered locations, skipped
files, and cancelled partial output use `warning`; failures use `error`. Existing
messages and the clipboard wording “Copied <what>” are preserved.

Logging writes to stderr using the `[notifications]` prefix. A failed delivery
preserves history and must not generate another notification recursively. OS
notifications are deferred; no native permissions or bubbles are enabled.

## Projection and toast delivery

The `notifications` snapshot contains its version and newest-first entries.
Ordered patches atomically add an entry and remove evicted IDs, or clear history.
Subscription and initial history capture share an atomic boundary. Frontend
projections ignore stale or repeated patch versions.

Only live added entries requesting `toast` join the frontend FIFO toast queue.
Each toast displays for 2.4 seconds, then expiry removes that exact active ID.
New arrivals never replace or restart the active toast. Snapshots and reloads
restore history without replaying toasts. Eviction and clearing history leave
queued delivery intact.

Arrival, expiry, and clicking a toast preserve existing focus. Toasts remain
informational live regions and do not acquire keyboard ownership.

## History UI

The existing `j` Jobs modal gains a Notifications tab; Jobs remains its initial
tab. History shows newest entries first, including severity, complete message,
and local date/time to seconds. The machine-readable timestamp remains UTC.

The tablist and notification list each expose one Tab stop. Tablist arrows and
Home/End select tabs; list arrows and Home/End move the active entry and keep it
visible. Incoming publications preserve the entry being browsed. The list
container keeps focus when entries disappear. ModalStack owns isolation and
opener restoration; Escape or `j` closes the modal.

Clear history is disabled when empty or disconnected. While pending it is
disabled and labelled “Clearing…”. Backend failure appears inline. The clear action
hands the Clear button's focus to the persistent history list before disabling it.
The first entry arriving in a focused empty list becomes its browsed entry;
subsequent arrivals retain that entry and keep it visible.
Clearing affects retained history only, produces no notification, and leaves queued
toasts and diagnostic logs intact.

Search, severity filtering, unread counts, persistence, notification settings,
and native notification bubbles are outside this version.
