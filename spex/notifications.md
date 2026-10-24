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
clears it. The OS delivery preference is persisted in backend settings; no
notification history or preferences are stored in localStorage.
Notification publication and history housekeeping are excluded from the
persistent command journal, which records semantic user commands rather than
delivery chatter. Selected stderr logging remains a separate delivery channel.
Storage and delivery have separate boundaries; delivery failure cannot undo a
recorded publication.

Remote activity also has a separate durable record in the existing SQLite
`journal_events` table, browsable through the read-only Journal source. Remote
jobs, pairing, and peer connection transitions record semantic lifecycle events
at their backend owner, independently of notification publication, GUI
connections, focus, OS delivery preferences, and execution-duration thresholds.
Clearing notification history does not delete Journal events. Publishing or
clearing notifications does not itself add Journal events. See
[remote-fbro.md](remote-fbro.md#durable-remote-activity-journal) for the lifecycle
and payload contract.

## Publishing and channels

`NotificationInput` contains `message`, optional `level` (`info`, `warning`, or
`error`), and optional `channels`. The ordinary defaults are `info` and
`toast`/`log`. An explicit empty channel list retains history without delivery.
Log-only publications also appear in history. Backend-selected operation events
may additionally request `os`; frontend `PublishNotification` requests remain
limited to `toast` and `log`, and reject `os` with `INVALID_NOTIFICATION`.

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
preserves history and must not generate another notification recursively.

## Background OS delivery

The backend owns native delivery and activity policy. Phase 1 supports Linux
through `notify-rust` and the session's freedesktop notification service, behind
the Platform Abstraction Layer. macOS and Windows adapters are unsupported until
their respective phases; the common event and settings contract is portable.

`general.osNotificationsEnabled` defaults to true and persists through backend
settings. The read-only `general.supportsOsNotifications` capability controls
visibility of the General settings checkbox. Disabling native delivery leaves
history and requested toast/log delivery intact.

OS delivery requires no focused native fbro window, including detached viewers
and diffs. Native focus/destruction events maintain this state; startup uncertainty
suppresses delivery. A 150 ms focus handoff grace period prevents alerts while
switching between fbro windows. Delivery rechecks activity and the preference
immediately before submission. Suppressed notifications are never replayed when
fbro loses focus or a setting changes.

| Event | OS eligibility while inactive |
|---|---|
| Copy completion/cancellation; search, Fedup, and hash-check completion/cancellation | At least 30 seconds of operation execution |
| Remote transfer start | Immediate |
| Remote transfer completion/cancellation | At least 30 seconds of transfer execution |
| Peer connection/disconnection in either direction | Per-peer, per-direction transition, coalesced over two seconds |
| Incoming pairing request | Immediate; expires with the handshake |
| Operation failure or hash mismatch | Immediate, regardless of duration |

Execution starts when a job runs, excluding queue time and including subsequent
pauses and decision waits. The threshold filters OS delivery, not the authoritative
history or existing in-app feedback. Publishers classify typed outcomes rather
than message text, commit final state/results first, and claim each lifecycle
publication once before GUI delivery. File errors and hash mismatches are
aggregated into operation summaries. Validation errors and diagnostic logs are
not automatically OS notifications.

Remote notifications aggregate connections per peer **within each direction** and
deduplicate transfer events by stable operation identity. Incoming and outgoing
connections have independent reported state and debounce generations: an open
outgoing mount must not hide a peer connecting to or disconnecting from local
shares, and an incoming session must not hide outgoing changes. Messages identify
the direction as incoming or outgoing. Copy and remote-transfer terminal feedback
on the initiating instance share one summary. Serving-side transfer completion
reuses the existing 1.5-second idle heuristic and may later resume; neutral
“transfer ended” wording does not claim verified completion on the receiving peer.
Pairing alerts contain the peer name, never the pairing code, and only incoming
requests qualify for OS delivery. The durable Journal records pairing requests
on both the initiating and receiving instances.

The native sink has a bounded queue of 128 pending deliveries; no native I/O runs
under notification history or shared-state locks. Linux notifications use fbro's
identity/icon, escaped body text, normal urgency, and desktop-controlled expiry. Missing session
services, delivery failures, or desktop suppression leave operation results and
history intact. Native delivery errors are logged without recursive publication
or stale retries. Native service calls time out after three seconds. The adapter
retains the newest 128 native handles, evicting older connections rather than
depending on native closure signals to accept future notifications. Queue overflow
logs a delivery error and leaves the canonical history entry intact.
Native UI remains informational: no activation, navigation, or
approval actions, and no forced focus or Do Not Disturb override.

Native delivery is available only while the desktop application is running.
After-exit delivery, push services, per-event preferences, and notification actions
are outside this phase. See [the phase roadmap](plans/os-notifications.md).

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

History search, severity filtering, unread counts, and history persistence are
outside this version.
