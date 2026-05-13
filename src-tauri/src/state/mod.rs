pub mod settings;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::bookmarks::BookmarkEntry;
use crate::bro::ResolvedBro;
use crate::jobs::search::SearchResultStore;
use crate::jobs::JobStore;
use crate::platform::{Platform, ResolvedOpenWithApp};
use crate::fileserver::ThumbCache;
use crate::thumb_pool::ThumbPool;
use crate::vfs::VfsDriver;
use crate::ws::protocol::{EntrySchema, FilterSpec, ServerMessage, SortSpec};

// ─── Source ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub supports: Vec<String>,
    pub root_path: String,
    pub mounted: bool,
    pub device_path: Option<String>,
    /// Shadow sources are transient per-panel sources (e.g. browsing inside a zip archive).
    /// They are not volumes and must not appear in the volume/source bar.
    pub shadow: bool,
}

// ─── Shadow source ────────────────────────────────────────────────────────────

/// Context stored on a panel while it is browsing inside a shadow filesystem
/// (e.g. a zip archive). Holds everything needed to return to the parent source.
#[derive(Clone, Debug)]
pub struct ShadowSource {
    /// The source_id the panel was using before entering the archive.
    pub parent_source_id: String,
    /// The directory path the panel was in before entering the archive.
    pub parent_path: String,
    /// Absolute path to the archive file on the host filesystem.
    pub archive_path: String,
    /// Name of the archive file (used as cursor hint when exiting).
    pub cursor_hint: Option<String>,
    /// Cursor index to restore when exiting (used instead of cursor_hint when
    /// the parent has duplicate entry names, e.g. search results).
    pub cursor_index: Option<usize>,
    /// When entering a zip from within another shadow source (e.g. search results),
    /// the previous shadow context is stashed here so it can be restored on exit.
    pub parent_shadow: Option<Box<ShadowSource>>,
}

// ─── Panel ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelViewMode {
    List,
    Grid,
}

impl PanelViewMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PanelViewMode::List => "list",
            PanelViewMode::Grid => "grid",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "list" => Some(PanelViewMode::List),
            "grid" => Some(PanelViewMode::Grid),
            _ => None,
        }
    }
}

impl Default for PanelViewMode {
    fn default() -> Self {
        PanelViewMode::List
    }
}

// ─── Tabs / Slots ─────────────────────────────────────────────────────────────

/// What kind of content a tab carries. Listing is the default folder view;
/// Terminal is a promoted PTY session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Listing,
    Terminal,
}

impl TabKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TabKind::Listing => "listing",
            TabKind::Terminal => "terminal",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "listing" => Some(TabKind::Listing),
            "terminal" => Some(TabKind::Terminal),
            _ => None,
        }
    }
}

impl Default for TabKind {
    fn default() -> Self {
        TabKind::Listing
    }
}

/// A slot in the panel layout. Each slot hosts one or more tabs (panels).
/// A single-tab slot renders identically to a pre-tabs panel — the FE hides
/// the tab bar until `tab_panel_ids.len() > 1`.
#[derive(Clone, Debug)]
pub struct PanelSlot {
    pub slot_id: String,
    pub tab_panel_ids: Vec<String>,
    pub active_panel_id: String,
}

#[derive(Clone, Debug)]
pub struct Panel {
    pub panel_id: String,
    /// Slot this panel belongs to. Panels (tabs) sharing a slot are siblings.
    pub slot_id: String,
    /// Tab kind (listing or terminal). Default Listing.
    pub tab_kind: TabKind,
    /// User-supplied tab title. When None, the FE derives the title from
    /// the folder name (or pty cwd for terminal tabs).
    pub tab_title_override: Option<String>,
    pub source_id: String,
    pub path: String,
    pub status: PanelStatus,
    pub sort: SortSpec,
    pub filter: Option<FilterSpec>,
    pub filter_mode_active: bool,
    pub show_hidden: bool,
    pub view_mode: PanelViewMode,
    pub cursor: usize,
    pub selection: Vec<usize>,
    pub entries: Vec<EntrySchema>,
    pub version: u64,
    pub calculating_size: HashSet<String>,
    /// When true, this panel is in windowed mode (Mode B) — FE receives only a
    /// slice of entries rather than the full list.
    pub windowed: bool,
    pub window_start: usize,
    pub window_count: usize,
    /// Monotonic counter for window-slice patches. Decoupled from `version`
    /// so render-slice updates from `send_window` can't be dropped by the FE
    /// version guard when interleaved with structural patches.
    pub window_version: u64,
    /// Last visited path per source — used to restore position when switching back.
    pub source_paths: HashMap<String, String>,
    /// Set when the panel is browsing inside a shadow filesystem (e.g. zip archive).
    pub shadow_source: Option<ShadowSource>,
    /// Set when the panel hosts an embedded terminal split. Holds the PTY session id.
    pub terminal_session: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelStatus {
    Loading,
    Ready,
    Error(String),
}

impl PanelStatus {
    pub fn as_str(&self) -> &str {
        match self {
            PanelStatus::Loading => "loading",
            PanelStatus::Ready => "ready",
            PanelStatus::Error(_) => "error",
        }
    }
}

// ─── AppState ────────────────────────────────────────────────────────────────

pub struct AppState {
    pub panels: HashMap<String, Panel>,
    /// Slots in the panel layout. Each slot owns 1+ panels (tabs).
    pub slots: HashMap<String, PanelSlot>,
    /// Ordered slot ids — drives the visible layout left-to-right.
    pub layout_slots: Vec<String>,
    pub sources: HashMap<String, Source>,
    pub drivers: HashMap<String, Arc<dyn VfsDriver>>,
    pub focus: String,
    pub platform: Arc<dyn Platform>,
    pub job_store: JobStore,
    pub open_with_requests: HashMap<String, OpenWithRequest>,
    pub bro_requests: HashMap<String, BroRequest>,
    /// Live search result stores, keyed by source_id ("search:<job_id>").
    pub search_stores: HashMap<String, Arc<Mutex<SearchResultStore>>>,
    /// Senders for all connected GUI clients — used to broadcast state updates
    /// from non-GUI connections (e.g. viewer windows).
    pub gui_senders: Vec<mpsc::Sender<ServerMessage>>,
    /// Port of the local HTTP file server used by viewer windows for media streaming.
    pub file_server_port: u16,
    /// Count of currently active viewers (external viewer windows + embedded
    /// viewer panels). Used to gate background prefetch on cursor movement so
    /// plain panel navigation doesn't warm the cache unnecessarily.
    pub active_viewers: usize,
    /// Set to true after the first GUI client connects and completes its
    /// initial scan. Used by the server to skip the clear-and-rescan cycle
    /// on reconnection, keeping existing entries visible during the refresh.
    pub has_gui_connected: bool,
    pub bookmarks: Vec<BookmarkEntry>,
    pub font_size: u8,
    pub settings: settings::Settings,
    pub child_processes: HashMap<u32, ChildProcessEntry>,
    pub pty_manager: Arc<crate::terminal::PtyManager>,
    /// Thumbnail generation worker pool. The fileserver uses it directly to
    /// service `/__thumb__/` requests; runtime helpers push the
    /// currently-visible path set to it on entries / window changes so
    /// workers prioritize cards the user is actually looking at.
    pub thumb_pool: Arc<ThumbPool>,
    /// In-memory L1 thumbnail cache. Held here so the settings handler can
    /// invalidate it when the user clicks "Clear cache".
    pub thumb_cache: Arc<ThumbCache>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChildProcessEntry {
    pub pid: u32,
    pub name: String,
    pub label: String,
    pub started_at: u64,
}

#[allow(dead_code)]
pub struct OpenWithRequest {
    pub path: String,
    pub kind: String,
    pub apps: Vec<ResolvedOpenWithApp>,
}

pub struct BroRequest {
    pub source_panel_id: String,
    pub result_panel_id: Option<String>,
    /// Stored so inputs can be re-resolved in full at launch time.
    pub target_path: Option<String>,
    pub bros: Vec<ResolvedBro>,
}

impl AppState {
    pub fn new_default(
        platform: Arc<dyn Platform>,
        initial_paths: HashMap<String, String>,
        file_server_port: u16,
        thumb_pool: Arc<ThumbPool>,
        thumb_cache: Arc<ThumbCache>,
    ) -> Self {
        let home = platform.home_dir();
        let loaded_settings = settings::load();
        let show_hidden_default = loaded_settings.general.show_hidden_default;

        // Validate a saved path; fall back to home if it is no longer a directory.
        let resolve = |id: &str| -> String {
            initial_paths
                .get(id)
                .filter(|p| Path::new(p.as_str()).is_dir())
                .cloned()
                .unwrap_or_else(|| home.clone())
        };

        let supports = vec![
            "ListEntries".to_string(),
            "OpenFolder".to_string(),
            "CopySelection".to_string(),
            "MoveSelection".to_string(),
            "DeleteSelection".to_string(),
            "InspectItem".to_string(),
            "Rename".to_string(),
            "Edit".to_string(),
            "Touch".to_string(),
            "NewFolder".to_string(),
            "OpenShell".to_string(),
        ];

        // Detect volumes from platform; fall back to a single root volume if empty.
        let mut vols = platform.list_volumes();
        if vols.is_empty() {
            vols.push(crate::platform::VolumeInfo {
                id: "vol-root".to_string(),
                name: "/".to_string(),
                root_path: "/".to_string(),
                mounted: true,
                device_path: None,
            });
        }

        // Determine which source to use for the initial panels: pick the volume
        // whose root_path is the longest prefix of home_dir.
        let initial_source_id = vols
            .iter()
            .filter(|v| home.starts_with(&v.root_path))
            .max_by_key(|v| v.root_path.len())
            .map(|v| v.id.clone())
            .unwrap_or_else(|| vols[0].id.clone());

        let default_sort = SortSpec {
            by: "name".to_string(),
            order: "asc".to_string(),
        };

        let view_for = |id: &str| -> PanelViewMode {
            if loaded_settings.general.save_panel_view_mode {
                loaded_settings
                    .panel_view_modes
                    .get(id)
                    .and_then(|s| PanelViewMode::from_str(s))
                    .unwrap_or_default()
            } else {
                PanelViewMode::default()
            }
        };

        let left = Panel {
            panel_id: "left".to_string(),
            slot_id: "left".to_string(),
            tab_kind: TabKind::Listing,
            tab_title_override: None,
            source_id: initial_source_id.clone(),
            path: resolve("left"),
            status: PanelStatus::Loading,
            sort: default_sort.clone(),
            filter: None,
            filter_mode_active: false,
            show_hidden: show_hidden_default,
            view_mode: view_for("left"),
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
            calculating_size: HashSet::new(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: HashMap::new(),
            shadow_source: None,
            terminal_session: None,
        };

        let right = Panel {
            panel_id: "right".to_string(),
            slot_id: "right".to_string(),
            tab_kind: TabKind::Listing,
            tab_title_override: None,
            source_id: initial_source_id.clone(),
            path: resolve("right"),
            status: PanelStatus::Loading,
            sort: default_sort,
            filter: None,
            filter_mode_active: false,
            show_hidden: show_hidden_default,
            view_mode: view_for("right"),
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
            calculating_size: HashSet::new(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: HashMap::new(),
            shadow_source: None,
            terminal_session: None,
        };

        let mut panels = HashMap::new();
        panels.insert("left".to_string(), left);
        panels.insert("right".to_string(), right);

        // Seed one single-tab slot per panel. Slot id mirrors the panel id
        // so existing single-panel sessions round-trip identically.
        let mut slots: HashMap<String, PanelSlot> = HashMap::new();
        slots.insert(
            "left".to_string(),
            PanelSlot {
                slot_id: "left".to_string(),
                tab_panel_ids: vec!["left".to_string()],
                active_panel_id: "left".to_string(),
            },
        );
        slots.insert(
            "right".to_string(),
            PanelSlot {
                slot_id: "right".to_string(),
                tab_panel_ids: vec!["right".to_string()],
                active_panel_id: "right".to_string(),
            },
        );
        let layout_slots = vec!["left".to_string(), "right".to_string()];

        // Build sources and drivers from volumes.
        #[cfg(unix)]
        let is_spy = std::env::var("FBRO_SPY").is_ok();
        #[cfg(unix)]
        let sock_path =
            std::env::var("FBRO_SPY_SOCK").unwrap_or_else(|_| "/tmp/fbro-spy.sock".to_string());

        let mut sources = HashMap::new();
        let mut drivers: HashMap<String, Arc<dyn VfsDriver>> = HashMap::new();

        for vol in vols {
            let source = Source {
                id: vol.id.clone(),
                name: vol.name,
                kind: "filesystem".to_string(),
                supports: if vol.mounted {
                    supports.clone()
                } else {
                    vec![]
                },
                root_path: vol.root_path,
                mounted: vol.mounted,
                device_path: vol.device_path,
                shadow: false,
            };
            if vol.mounted {
                let driver: Arc<dyn VfsDriver> = {
                    #[cfg(unix)]
                    if is_spy {
                        let inner = Arc::new(crate::vfs::local::LocalFsDriver);
                        Arc::new(crate::vfs::spy::SpyFsDriver::new(inner, sock_path.clone()))
                    } else {
                        Arc::new(crate::vfs::local::LocalFsDriver)
                    }
                    #[cfg(not(unix))]
                    Arc::new(crate::vfs::local::LocalFsDriver)
                };
                drivers.insert(vol.id.clone(), driver);
            }
            sources.insert(vol.id.clone(), source);
        }

        sources.insert(
            "procs".to_string(),
            Source {
                id: "procs".to_string(),
                name: "procs".to_string(),
                kind: "virtual".to_string(),
                supports: vec!["ListEntries".to_string(), "DeleteSelection".to_string()],
                root_path: String::new(),
                mounted: true,
                device_path: None,
                shadow: false,
            },
        );
        drivers.insert(
            "procs".to_string(),
            Arc::new(crate::vfs::procs::ProcsDriver),
        );

        AppState {
            panels,
            slots,
            layout_slots,
            sources,
            drivers,
            focus: "left".to_string(),
            platform,
            job_store: JobStore::new(),
            open_with_requests: HashMap::new(),
            bro_requests: HashMap::new(),
            search_stores: HashMap::new(),
            gui_senders: Vec::new(),
            file_server_port,
            active_viewers: 0,
            has_gui_connected: false,
            bookmarks: crate::bookmarks::load(),
            font_size: crate::persistence::load().font_size.unwrap_or(13),
            settings: loaded_settings,
            child_processes: HashMap::new(),
            pty_manager: Arc::new(crate::terminal::PtyManager::new()),
            thumb_pool,
            thumb_cache,
        }
    }

    /// Rebuild sources and drivers from a new volume list.
    /// Returns IDs of sources that were removed.
    pub fn rebuild_sources(&mut self, volumes: Vec<crate::platform::VolumeInfo>) -> Vec<String> {
        let supports = vec![
            "ListEntries".to_string(),
            "OpenFolder".to_string(),
            "CopySelection".to_string(),
            "MoveSelection".to_string(),
            "DeleteSelection".to_string(),
            "InspectItem".to_string(),
            "Rename".to_string(),
            "Edit".to_string(),
            "Touch".to_string(),
            "NewFolder".to_string(),
            "OpenShell".to_string(),
        ];

        #[cfg(unix)]
        let is_spy = std::env::var("FBRO_SPY").is_ok();
        #[cfg(unix)]
        let sock_path =
            std::env::var("FBRO_SPY_SOCK").unwrap_or_else(|_| "/tmp/fbro-spy.sock".to_string());

        let new_ids: HashSet<String> = volumes.iter().map(|v| v.id.clone()).collect();
        let old_ids: HashSet<String> = self
            .sources
            .iter()
            .filter(|(_, src)| !src.shadow)
            .map(|(id, _)| id.clone())
            .collect();

        // Remove stale sources/drivers (never touch shadow sources — they have their own lifecycle)
        let removed: Vec<String> = old_ids.difference(&new_ids).cloned().collect();
        for id in &removed {
            self.sources.remove(id);
            self.drivers.remove(id);
        }

        // Add or update sources/drivers
        for vol in volumes {
            let make_driver = || -> Arc<dyn VfsDriver> {
                #[cfg(unix)]
                if is_spy {
                    let inner = Arc::new(crate::vfs::local::LocalFsDriver);
                    return Arc::new(crate::vfs::spy::SpyFsDriver::new(inner, sock_path.clone()));
                }
                Arc::new(crate::vfs::local::LocalFsDriver)
            };

            if let Some(existing) = self.sources.get_mut(&vol.id) {
                // Update existing source if mount status or path changed
                if existing.mounted != vol.mounted || existing.root_path != vol.root_path {
                    existing.mounted = vol.mounted;
                    existing.root_path = vol.root_path;
                    existing.device_path = vol.device_path;
                    existing.name = vol.name;
                    if vol.mounted {
                        existing.supports = supports.clone();
                        if !self.drivers.contains_key(&existing.id) {
                            self.drivers.insert(existing.id.clone(), make_driver());
                        }
                    } else {
                        existing.supports.clear();
                        self.drivers.remove(&existing.id);
                    }
                }
            } else {
                // New source
                let source = Source {
                    id: vol.id.clone(),
                    name: vol.name,
                    kind: "filesystem".to_string(),
                    supports: if vol.mounted {
                        supports.clone()
                    } else {
                        vec![]
                    },
                    root_path: vol.root_path,
                    mounted: vol.mounted,
                    device_path: vol.device_path,
                    shadow: false,
                };
                if vol.mounted {
                    self.drivers.insert(vol.id.clone(), make_driver());
                }
                self.sources.insert(vol.id.clone(), source);
            }
        }

        removed
    }
}

/// Add restored tabs (beyond the first, which `new_default` already seeded)
/// to each slot. Drops tabs whose source isn't currently mounted, falling
/// back to home. Newly-added panels go into AppState.panels in Loading state
/// — the WS server's initial-scan loop picks them up on first GUI connect.
pub fn restore_tabs_from_session(
    state: &mut AppState,
    session_slots: &[crate::persistence::SlotSession],
) {
    use crate::persistence::TabSession;

    if session_slots.is_empty() {
        return;
    }

    let home = state.platform.home_dir();
    let show_hidden_default = state.settings.general.show_hidden_default;
    let default_sort = crate::ws::protocol::SortSpec {
        by: "name".to_string(),
        order: "asc".to_string(),
    };

    let pick_source = |source_id: &Option<String>| -> Option<String> {
        if let Some(sid) = source_id {
            if state.sources.contains_key(sid) && state.drivers.contains_key(sid) {
                return Some(sid.clone());
            }
        }
        None
    };

    for slot_session in session_slots {
        // Only act on slots that already exist (left/right) — sessions written
        // by future multi-slot builds shouldn't accidentally clobber state.
        let Some(slot) = state.slots.get(&slot_session.slot_id).cloned() else {
            continue;
        };
        if slot_session.tabs.is_empty() {
            continue;
        }

        // The first tab corresponds to the existing seed panel — set its
        // path/source from the saved tab so the home-fallback case lines up.
        if let Some(first) = slot_session.tabs.first() {
            let seed_panel_id = slot.tab_panel_ids[0].clone();
            if let Some(panel) = state.panels.get_mut(&seed_panel_id) {
                match first {
                    TabSession::Listing { source_id, path } => {
                        let resolved = pick_source(source_id)
                            .unwrap_or_else(|| panel.source_id.clone());
                        let resolved_path = if Path::new(path).is_dir() {
                            path.clone()
                        } else {
                            home.clone()
                        };
                        panel.source_id = resolved;
                        panel.path = resolved_path;
                        panel.tab_kind = TabKind::Listing;
                    }
                    TabSession::Terminal { cwd } => {
                        // Terminal tabs don't restore as live PTYs — fall back
                        // to a plain listing at the recorded cwd.
                        let resolved_path = if Path::new(cwd).is_dir() {
                            cwd.clone()
                        } else {
                            home.clone()
                        };
                        panel.path = resolved_path;
                        panel.tab_kind = TabKind::Listing;
                    }
                }
            }
        }

        // Build the additional tabs (index 1..). Allocate fresh panel ids
        // inside this slot.
        let mut added_ids: Vec<String> = Vec::new();
        for tab in slot_session.tabs.iter().skip(1) {
            let (path, source_id) = match tab {
                TabSession::Listing { source_id, path } => {
                    let path = if Path::new(path).is_dir() {
                        path.clone()
                    } else {
                        home.clone()
                    };
                    let source_id = pick_source(source_id).unwrap_or_else(|| {
                        // Inherit source from the seed panel as a last resort.
                        state
                            .panels
                            .get(&slot.tab_panel_ids[0])
                            .map(|p| p.source_id.clone())
                            .unwrap_or_default()
                    });
                    (path, source_id)
                }
                TabSession::Terminal { cwd } => {
                    let path = if Path::new(cwd).is_dir() {
                        cwd.clone()
                    } else {
                        home.clone()
                    };
                    let source_id = state
                        .panels
                        .get(&slot.tab_panel_ids[0])
                        .map(|p| p.source_id.clone())
                        .unwrap_or_default();
                    (path, source_id)
                }
            };

            if source_id.is_empty() {
                continue;
            }

            // Allocate a panel id within the slot.
            let mut n = 1u64;
            let new_panel_id = loop {
                let candidate = format!("{}-tab-{}", slot.slot_id, n);
                if !state.panels.contains_key(&candidate) {
                    break candidate;
                }
                n += 1;
            };

            let new_panel = Panel {
                panel_id: new_panel_id.clone(),
                slot_id: slot.slot_id.clone(),
                tab_kind: TabKind::Listing,
                tab_title_override: None,
                source_id,
                path,
                status: PanelStatus::Loading,
                sort: default_sort.clone(),
                filter: None,
                filter_mode_active: false,
                show_hidden: show_hidden_default,
                view_mode: PanelViewMode::List,
                cursor: 0,
                selection: vec![],
                entries: vec![],
                version: 0,
                calculating_size: HashSet::new(),
                windowed: false,
                window_start: 0,
                window_count: 0,
                window_version: 0,
                source_paths: HashMap::new(),
                shadow_source: None,
                terminal_session: None,
            };
            state.panels.insert(new_panel_id.clone(), new_panel);
            added_ids.push(new_panel_id);
        }

        // Wire the tabs into the slot, preserving active_index.
        if let Some(live_slot) = state.slots.get_mut(&slot_session.slot_id) {
            for id in &added_ids {
                live_slot.tab_panel_ids.push(id.clone());
            }
            let active_index = slot_session
                .active_index
                .min(live_slot.tab_panel_ids.len().saturating_sub(1));
            live_slot.active_panel_id = live_slot.tab_panel_ids[active_index].clone();
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

/// Walk all panels and collect the set of host file paths currently in any
/// grid-mode panel's visible window. Used to drive thumbnail-generation
/// priority — only entries the user is actually looking at get the high
/// queue. Caller holds the AppState lock.
pub fn collect_visible_thumb_paths(state: &AppState) -> std::collections::HashSet<String> {
    let mut paths = std::collections::HashSet::new();
    for panel in state.panels.values() {
        if panel.view_mode != PanelViewMode::Grid {
            continue;
        }
        let total = panel.entries.len();
        let (start, end) = if panel.windowed {
            let s = panel.window_start.min(total);
            let e = (s + panel.window_count).min(total);
            (s, e)
        } else {
            (0, total)
        };
        let dir = panel.path.trim_end_matches(|c| c == '/' || c == '\\');
        for entry in &panel.entries[start..end] {
            if entry.kind != "file" {
                continue;
            }
            if let Some(sp) = &entry.source_path {
                paths.insert(sp.clone());
            } else {
                paths.insert(format!("{}/{}", dir, entry.name));
            }
        }
    }
    paths
}
