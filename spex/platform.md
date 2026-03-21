# fbro — Platform Abstraction Layer (PAL)

The Platform Abstraction Layer is a BE-internal subsystem that resolves
platform-specific environment attributes such as user home directories,
standard paths, and OS capabilities.

---

## 1. Purpose

Different operating systems expose environment information in different ways.
The PAL hides these differences behind a uniform trait, so the rest of the BE
(state initialisation, intent handlers, VFS drivers) can request platform
attributes without branching on the current OS.

---

## 2. Design Principles

* **BE-internal** — the FE never queries platform attributes directly.
* **Injected** — the concrete implementation is wired in `main.rs` and passed
  to `AppState`; all consumers receive it through shared state.
* **Single trait, multiple implementations** — one implementation per target
  platform (e.g. `LinuxPlatform`, `MacOsPlatform`, `WindowsPlatform`).
* **No filesystem mutations** — the PAL only reads environment data; it never
  modifies anything.

---

## 3. `Platform` Trait

```rust
pub trait Platform: Send + Sync {
    /// Absolute path to the current user's home directory.
    fn home_dir(&self) -> String;
}
```

Additional methods will be added as needed (e.g. `config_dir`, `cache_dir`,
`trash_dir`, `os_name`).

---

## 4. Implementations

### 4.1 `LinuxPlatform`

Covers Linux (and any POSIX system that follows the same conventions).

| Step | Source | Condition |
|------|--------|-----------|
| 1 | `$HOME` environment variable | Non-empty value |
| 2 | `/proc/self/status` → `Uid:` field → `/etc/passwd` lookup | `$HOME` absent or empty |
| 3 | `/` | All of the above fail |

Resolution order follows the POSIX convention: the `$HOME` variable is
authoritative and is always tried first.

---

### 4.2 Future platforms

| Platform | Implementation | Notes |
|----------|---------------|-------|
| macOS | `MacOsPlatform` | `$HOME` / `NSHomeDirectory` |
| Windows | `WindowsPlatform` | `USERPROFILE` / `SHGetKnownFolderPath` |

---

## 5. Wiring

`AppState` holds an `Arc<dyn Platform>` field. `AppState::new_default` receives
the implementation at construction time:

```rust
// main.rs
let platform = Arc::new(LinuxPlatform);
let shared_state = Arc::new(Mutex::new(AppState::new_default(platform)));
```

This means the platform implementation can be swapped in tests or for other
targets without touching any application logic.

---

## 6. Intents that consume PAL

| Intent | PAL method used | Effect |
|--------|----------------|--------|
| `NavigateHome` | `home_dir()` | Navigates the target panel to the user's home directory |

As more platform-aware intents are added (e.g. `NavigateDesktop`,
`NavigateDownloads`) the table above grows accordingly.

---

## 7. `NavigateHome` intent

Navigates a panel to the platform-resolved home directory.

```json
{
  "type": "Intent",
  "name": "NavigateHome",
  "scope": "<panelId>"
}
```

**BE behaviour**

1. Resolve `scope` to a panel ID.
2. Call `platform.home_dir()`.
3. Call `navigate_to(panel_id, home_path)` — identical to any other directory
   navigation: clears entries and selection, resets cursor, starts a new scan
   job, sends a `StatePatch`.

**FE trigger**

A `~` button in the panel header fires this intent with the panel's own ID as
scope. The button carries `tabIndex={-1}` so it never steals keyboard focus
from the panel.
