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

## Phase 2 — macOS

Implement the existing PAL contract through Notification Center's
`UNUserNotificationCenter`, using existing Rust bindings such as
`objc2-user-notifications`. Handle actual authorization and denial, requesting
permission while fbro is foregrounded; do not infer authorization from an API
that always reports granted. Verify packaged `.app` identity, the shared
all-window activity policy, system notification settings, and nonintrusive
delivery. Enable the capability/settings control only with the native adapter.

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
