# OS notification rollout

The shared architecture and Linux behavior are specified in
[notifications.md](../notifications.md), [platform.md](../platform.md), and
[settings.md](../settings.md). This document tracks the phased rollout, not a
second source of runtime policy.

## Phase 1 — Linux

Extend backend-owned history and channel routing with backend-selected `os`
delivery. Use `notify-rust` through the PAL, native activity across every fbro
window, the persistent default-on preference, and one bounded delivery worker.
Keep ordinary frontend publication defaults at toast/log, and reject frontend
requests for `os`.

Cover copy/search/Fedup/hash-check terminal outcomes, remote transfer start/end,
both directions of logical peer connections, incoming pairing, operation failures,
and hash mismatches. Successful and cancelled completions require at least 30
seconds of execution; failures and remote attention/start events bypass that
cutoff. Deduplicate at the authoritative publisher boundary and retain history
regardless of native delivery. Keep notifications informational and preserve
existing remote completion semantics with neutral end wording.

Validate policy with fake native delivery, isolated Linux service fixtures,
settings/history smoke cases, and real packaged desktop checks on Wayland/X11.
macOS and Windows remain unsupported during this phase.

## Phase 2 — macOS (implemented; native acceptance pending)

The PAL adapter uses `objc2-user-notifications` and Notification Center's
`UNUserNotificationCenter`. Foreground UI hooks asynchronously request alert
permission; worker delivery queries actual authorization and rechecks activity
after native IPC. Capability is enabled only for the packaged `dev.fbro.app`
bundle; bare development/test executables stay unsupported. Denial and native
errors preserve history, and system settings remain authoritative.
Packaging explicitly signs the complete app bundle (ad hoc by default), rather
than leaving only the linker's executable signature. The native smoke rejects an
unbound bundle signature before launching; its optional permission mode checks
the real native completion so an authorization error cannot pass as delivery
support merely because the capability flag is true.

Unit coverage includes identity validation, native plain-text payloads and unique
identifiers, denied/undetermined/unknown authorization, owned completion errors,
foreground/enabled permission preparation, and stale deliveries during native
authorization lookup. Before release, complete the packaged macOS acceptance
checklist in [TESTING.md](../../TESTING.md#native-os-notification-validation),
including real permission grant/denial, icon, all-window focus, and Do Not Disturb.

## Phase 3 — Windows

Implement the PAL adapter using `notify-rust`'s Windows backend with fbro's
installed application identity. Validate name/icon, installed MSI/NSIS package
behavior, system notification settings, and Windows 10/11 delivery. Development
executables do not replace installed-package checks. Retain the same shared
event, duration, focus, and settings contract.

## Deferred capabilities

After-exit delivery, push services, per-event preferences, notification activation
or navigation actions, and an explicit receiver-verified remote completion
handshake are outside this rollout. Persistent history is a separate storage
feature; only the OS preference persists in this phase.
