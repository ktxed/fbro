use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::jobs::JobStore;
use crate::platform::Platform;
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
    pub cursor: usize,
    pub selection: Vec<usize>,
    pub entries: Vec<EntrySchema>,
    pub version: u64,
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
    /// Senders for all connected GUI clients — used to broadcast state updates
    /// from non-GUI connections (e.g. viewer windows).
    pub gui_senders: Vec<mpsc::Sender<ServerMessage>>,
}

impl AppState {
    pub fn new_default(platform: Arc<dyn Platform>, initial_paths: HashMap<String, String>) -> Self {
        let home = platform.home_dir();

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
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
        };

        let right = Panel {
            panel_id: "right".to_string(),
            source_id: initial_source_id.clone(),
            path: resolve("right"),
            status: PanelStatus::Loading,
            sort: default_sort,
            filter: None,
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
        };

        let mut panels = HashMap::new();
        panels.insert("left".to_string(), left);
        panels.insert("right".to_string(), right);

        // Build sources and drivers from volumes.
        let is_spy = std::env::var("FBRO_SPY").is_ok();
        let sock_path = std::env::var("FBRO_SPY_SOCK")
            .unwrap_or_else(|_| "/tmp/fbro-spy.sock".to_string());

        let mut sources = HashMap::new();
        let mut drivers: HashMap<String, Arc<dyn VfsDriver>> = HashMap::new();

        for vol in vols {
            let source = Source {
                id: vol.id.clone(),
                name: vol.name,
                kind: "filesystem".to_string(),
                supports: supports.clone(),
                root_path: vol.root_path,
            };
            let driver: Arc<dyn VfsDriver> = if is_spy {
                let inner = Arc::new(crate::vfs::local::LocalFsDriver);
                Arc::new(crate::vfs::spy::SpyFsDriver::new(inner, sock_path.clone()))
            } else {
                Arc::new(crate::vfs::local::LocalFsDriver)
            };
            sources.insert(vol.id.clone(), source);
            drivers.insert(vol.id, driver);
        }

        AppState {
            panels,
            sources,
            drivers,
            focus: "left".to_string(),
            platform,
            job_store: JobStore::new(),
            gui_senders: Vec::new(),
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;
