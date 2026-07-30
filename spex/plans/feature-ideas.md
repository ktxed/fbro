# fbro — Product analysis & feature ideas

High-level product read of fbro as it exists today, plus a backlog of feature ideas ranked by strategic fit. This is a product-owner document, not an implementation ticket.

---

## 1. What fbro is today

**fbro** is not another pretty file browser. It is a **backend-authoritative orthodox file manager kernel**: dual panels, intent → job → decision pipeline, multi-source VFS, and a React shell that is mostly a projection of state.

That architecture is the product’s real moat. Most OFMs are UI-first with shell hacks. fbro is **ops-first with a pluggable source model**:

| Pillar | Reality today |
|--------|----------------|
| Orthodox DNA | Dual pane, F-keys, selection, copy/move/delete, archives-as-folders |
| Kernel | WS intents, BE state, interruptible jobs, capability-gated sources |
| Sources | Local, zip/7z, search, bookmarks, history, global selections, procs, journal, remote SFTP peers |
| Processors | **Bros** (zip/7z/tar, fedup, sync, media, kill-proc) |
| Power extras | Embedded PTY + quick edit, rich viewer (hex/md/mermaid/media), git chrome, peer remote with integrity |
| Current motion | Deepening **in-panel git** (bubbles, armed rollback, modals, e2e) — power-user polish, not Finder-clone |

**Positioning thesis:** *The file manager for people who treat the filesystem as a workbench — developers, sysadmins, power users — with a modern job engine and peer-aware remote, not a desktop shell accessory.*

### Strengths to exploit

1. **VFS + capability flags** — new “places” are cheap.
2. **Jobs engine** — any long op (analyze, sync, pack, hash) has a home.
3. **Bros** — “do something smart with these files” without a plugin marketplace yet.
4. **Remote fbro stack** — integrity-aware LAN transfer is rare and ownable.
5. **Git-as-panel-chrome** — unusual for OFMs; aligns with developer audience.
6. **Latent layout model** — spec already dreams n-panel, tabs, virtual panels.

### Strategic gaps (vs TC / Double Commander / Far / MC)

- User configurability (keymaps, toolbar, columns, associations)
- Generic networking (S3 / WebDAV / SMB / arbitrary SSH) vs peer-only remote
- Multi-rename, true directory sync UI, tree panel
- End-user plugins (explicit non-goal for protocol v0 — treat carefully)
- Undo of FS ops (journal is audit, not reverse)
- N-panel still dual-slot in runtime

---

## 2. Strategy lenses (how to pick features)

Every idea below is scored against these lenses:

1. **Kernel leverage** — reuses VFS / jobs / bros / remote control paths
2. **Orthodox delight** — makes power users say “finally” without breaking muscle memory
3. **Differentiation** — hard for Finder/Explorer/Nautilus; sticky vs TC/DC
4. **Audience fit** — developers + power users first (git, terminals, remotes, integrity)
5. **Cost realism** — cheap seam vs multi-quarter platform bet

**Recommended portfolio mix:** ~50% deepen differentiators, ~30% close OFM power gaps, ~20% speculative moonshots that only fbro’s kernel can own.

---

## 3. Feature ideas

### A. Deepen what only fbro already owns (high ROI, brand-defining)

#### A1. Integrity Transfer Manager

Productize the job queue into a first-class **Transfer & Integrity Manager**: BLAKE3 digests, resume, skip/pause, queue reorder, dry-run reports, “verify after copy,” and remote Sync Bro results in one place with history.

- **Why:** Jobs + digests + remote integrity already exist; UI productization is the gap.
- **Hook:** “Copy that finishes when hash says so.”
- **Cost:** Medium (UI + journal of transfer outcomes); architecture ready.

#### A2. Git Workbench Lite (not an IDE)

Ship the next 20% of git that lives *inside the panel*: stash, create/switch branch with dirty guards, log/show as virtual source or compare window, blame peek in viewer, “open changed files only” filter, multi-repo awareness when path is nested.

- **Why:** Current work already doubles down on inline git; stay OFM-native, refuse full merge/rebase IDE.
- **Hook:** “Your repo is a folder that fights back usefully.”
- **Cost:** Medium; CLI + existing git decorate + compare seams.

#### A3. Remote write + “Send to peer” tray

Promote remote from Settings-advanced to chrome: pair once, then **drag to peer**, “mirror selection to phone/laptop,” optional write when peer allows, and background receive inbox.

- **Why:** Peer SFTP + pairing is a differentiator; discoverability is weak.
- **Hook:** “AirDrop for people who check hashes.”
- **Cost:** Medium–high (write path, ACL on shared roots, UX).

#### A4. Bro Pack: smart processors as first-class product

Name and market the Bro layer: Fedup Bro, Sync Bro, Archive Bros, Media Bro — plus 3–5 new high-value bros (checksum report, image batch resize, “flatten folder,” “split by extension,” “encrypt vault zip”).

- **Why:** Bros are the plugin story without plugins; discovery today is F1/modal.
- **Hook:** “Pass to Bro” becomes a brand verb.
- **Cost:** Low–medium per bro; packaging/UX is the product work.

#### A5. Session & layout snapshots

Save named dual-panel sessions: paths, tabs, view modes, filters, even embedded terminal cwd. Restore “Work,” “Downloads vs NAS,” “Repo + build.”

- **Why:** Persistence pieces exist; “workspaces” is classic power-user gold.
- **Cost:** Low–medium.

---

### B. Close classic OFM power gaps (retention / table-stakes for TC migrants)

#### B1. Multi-Rename Studio

Mask-based rename: `[N]`, counters, case, regex replace, dry-run table, undo of rename job only.

- **Why:** Top TC/DC feature; jobs + conflict model fit dry-run perfectly.
- **Cost:** Medium.

#### B2. Directory Synchronize (two-pane mirror)

Not just Sync Bro integrity verify — a TC-style sync: compare trees, show newer/missing/size-diff, selective copy both ways, optional delete orphans with confirm.

- **Why:** Power users live here; fbro already has integrity compare seed.
- **Cost:** High product design; medium–high engineering on jobs.

#### B3. User keymaps + button bar

Settings UI over existing `default_bindings` schema; custom toolbar buttons that fire commands/bros with args.

- **Why:** Schema exists; macOS F-key remaps already prove need.
- **Cost:** Medium UX; low BE.

#### B4. Quick filter / typeahead jump modes

Separate **instant letter-jump** (TC) from **filter mode** (`f`), document bindings, optional always-on search bar.

- **Why:** Mental model confusion is a documented UX risk.
- **Cost:** Low.

#### B5. Custom columns & color-by-extension

Size/date already; add optional columns (permissions, owner, git status, MIME) and per-extension coloring rules in themes.

- **Why:** OFM identity; FE list rendering is the main work.
- **Cost:** Medium.

#### B6. Tree pane (optional third surface)

Collapsible directory tree for focused panel — or a true third “nav” slot as n-panel baby step.

- **Why:** Spec wants n-panel; tree is the safest first extra surface.
- **Cost:** Medium.

---

### C. New sources that ride the VFS (cheap, delightful, on-brand)

#### C1. Trash / Recycle source

Browse OS trash, restore, empty — capability-gated virtual source.

#### C2. Recent & Frequent

Virtual sources from journal + history: last opened, last edited, most visited folders.

#### C3. Tags / Labels (local DB)

User tags on paths; filter and `tag:foo` source. Journal/SQLite patterns already exist.

#### C4. Containers as folders

tar.gz/bz2/xz browse VFS (mirror zip/7z); later ISO, AppImage, Docker layer tars for the sysadmin crowd.

#### C5. “Clipboard FS” / Staging tray

Global selections are halfway there — productize as a **Staging Tray** with named bags, pin, export list, feed Fedup/Archive/Sync.

#### C6. Network seed (strategic, not cheap)

Arbitrary SFTP/SSH first (reuse remote stack partially), then WebDAV/S3 as drivers. Market as “same VFS, more places.”

- **Cost:** High; do after peer remote write UX is solid.

---

### D. Viewer, compare, and “see the work”

#### D1. Always-on preview strip

Optional right strip or bottom preview for focused file (image/text/media) without opening F3 — reuses file server + viewer components.

#### D2. Directory Diff as first-class window

Extend compare from files to folders with status chips (only left / only right / differ / same).

#### D3. Ludicrous → “Pro Viewer modes”

Package hex, mermaid, md, media into named modes; add SQLite browser, JSON tree, log tail with follow for large files.

#### D4. Side-by-side Edit (soft)

Not a full IDE: dual quick-edit PTYs or dual text buffers for “two config files” workflows.

---

### E. AI / automation (only where the kernel makes it safe)

Architecture note: BE-authoritative intents are **unusually AI-safe** — a client can propose intents; the BE validates capabilities and jobs.

#### E1. Command journal → “Recipes”

Record semantic journal entries into replayable recipes (rename patterns, bro chains, search → select → copy).

#### E2. Natural language → intent plan (gated)

“Find duplicates of these photos and stage the extras” → plan of bros/jobs shown for confirm. No silent FS mutation.

#### E3. Watch rules (local automation)

“When downloads gets a `.zip`, extract to Archive/YYYY-MM” as background jobs with confirm-on-first.

**Caution:** Do not lead with “chatbot in a file manager.” Lead with **confirmable automation** on the job engine.

---

### F. Polish & trust (ship confidence)

#### F1. Keyboard completeness pass

Modal Tab traps, Esc/Enter everywhere, in-app cheat sheet, palette grouping — `spex/keyboard.md` TODOs.

#### F2. First-run power tour

30-second tour: dual pane, F5 copy job, Bro, remote pair, git bubble — not a wall of text.

#### F3. Cross-platform parity sprint

Drag-out, thumbs, volume mount, bro set on macOS/Windows — “works on my Linux” is not a product.

#### F4. Tab rename + n-panel MVP

Protocol already has tab rename; n-panel as 2→3 slots proves the layout model.

#### F5. Soft undo for safe ops

Not full FS transactions — undo last rename/move within session when destination still exists (journal-assisted). Huge trust win.

---

## 4. Signature product bets

If the product needs a short, memorable roadmap narrative:

| Bet | Name | Why it defines fbro |
|-----|------|---------------------|
| **1** | **Integrity Transfer Manager + Remote write** | Owns “serious file movement” vs casual OS browsers |
| **2** | **Bro Pack + Multi-Rename + Dir Sync** | Owns “power OFM” for TC migrants with a modern job brain |
| **3** | **Git Workbench Lite + Staging Tray + Recipes** | Owns developer workbench without becoming VS Code |

Everything else is either table-stakes polish (keys, preview, trash) or strategic expansion (S3/SSH, plugins).

---

## 5. Suggested sequencing

### Now (high leverage, fits current motion)

1. Git Workbench Lite (stash, log/show source, dirty-path filter) — continues current branch of work
2. Keyboard + modal hygiene + cheat sheet — unlocks power-user conversion
3. Staging Tray productization of global selections
4. Transfer Manager UI (jobs + integrity history)
5. Tab rename + session snapshots

### Next

6. Multi-Rename Studio
7. Remote write + peer chrome (“Send to…”)
8. Directory Synchronize (build on Sync Bro)
9. Keymap + toolbar customization
10. Trash + Recent virtual sources; tar-as-VFS
11. Preview strip; directory diff window
12. Soft undo for rename/move

### Later / strategic

13. Arbitrary SFTP/SSH driver; then S3/WebDAV
14. N-panel layouts (3+); tree pane
15. Confirmable automation / recipes / optional NL planner
16. Real plugin API (only when bros + drivers demand user extensibility)

---

## 6. Ideas to park (and why)

| Idea | Why park it |
|------|-------------|
| Full IDE SCM (rebase, PR UI, GPG) | Different product; git-as-chrome is enough |
| Marketplace plugins | Protocol non-goal today; security + versioning tax |
| Cloud sync daemon | Conflict model swallows teams |
| Mobile-first FM | PAL/PTY/volumes are desktop-shaped |
| “AI chat that deletes files” | Trust destruction; only gated plans |
| Compete with Finder aesthetics | Lose; compete on ops + sources |

---

## 7. Creative moonshots (optional, high brand)

Not backlog commitments — illustrations of how far the **kernel** can go if product stays ops-centric:

1. **fbro Mesh** — three machines, one multi-source copy graph with per-hop integrity; “project folder follows me home.”
2. **Panel as REPL** — focused panel accepts a mini expression language (`size>10M & ext:png`) as filter, saved as virtual views.
3. **Time-machine panel** — snapshot-aware browse (btrfs/ZFS/.git/Time Machine) as VFS drivers: “folder at commit / snapshot.”
4. **Pair programming FS** — temporary shared writable remote root with presence + op journal for two humans (or human + agent).
5. **Job as artifact** — every large copy exports a signed transfer receipt (hashes, peers, times) for audit / legal / sysadmin.

---

## 8. Summary

**fbro already has the hard parts of a serious OFM kernel.** The opportunity is not “add more random features”; it is to:

1. **Name and productize** the differentiators (integrity jobs, bros, peer remote, git chrome).
2. **Fill OFM power gaps** that block TC/DC migration (multi-rename, dir sync, keymaps).
3. **Grow sources** ruthlessly through VFS (trash, recent, tar, later cloud).
4. **Stay allergic** to becoming a mediocre IDE, chat app, or Finder skin.

The best next features feel like inevitable consequences of the architecture — not bolted-on fads.

---

## Appendix — current surface (cheat sheet)

**Implemented:** dual panel, tabs, terminals/quick edit, jobs+conflicts, search-as-VFS, zip/7z, bros (incl. fedup/sync), git inline+modals, remote pair/browse/copy, viewer/diff, bookmarks/history/procs/journal/global selections, grid thumbs, settings, e2e smoke for core paths.

**Spec / incomplete:** n-panel, tab rename, keyboard modal polish, remote write, generic remotes, undo, plugins, user key rebinding UI.

**Related specs:** `spex/spec.md`, `spex/proto.md`, `spex/vfs.md`, `spex/bro.md`, `spex/remote-fbro.md`, `spex/keyboard.md`, `spex/tabs.md`, `spex/search.md`.
