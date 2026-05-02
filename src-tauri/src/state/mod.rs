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

#[derive(Clone, Debug)]
pub struct Panel {
    pub panel_id: String,
    pub source_id: String,
    pub path: String,
    pub status: PanelStatus,
    pub sort: SortSpec,
    pub filter: Option<FilterSpec>,
    pub filter_mode_active: bool,
    pub show_hidden: bool,
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

        let left = Panel {
            panel_id: "left".to_string(),
            source_id: initial_source_id.clone(),
            path: resolve("left"),
            status: PanelStatus::Loading,
            sort: default_sort.clone(),
            filter: None,
            filter_mode_active: false,
            show_hidden: show_hidden_default,
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
            source_id: initial_source_id.clone(),
            path: resolve("right"),
            status: PanelStatus::Loading,
            sort: default_sort,
            filter: None,
            filter_mode_active: false,
            show_hidden: show_hidden_default,
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

        AppState {
            panels,
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

pub type SharedState = Arc<Mutex<AppState>>;
