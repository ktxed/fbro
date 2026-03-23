use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::jobs::JobStore;
use crate::platform::Platform;
use crate::vfs::VfsDriver;
use crate::ws::protocol::{EntrySchema, FilterSpec, SortSpec};

// ─── Source ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub supports: Vec<String>,
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
        let source_id = "local-home".to_string();

        let source = Source {
            id: source_id.clone(),
            name: "Home".to_string(),
            kind: "filesystem".to_string(),
            supports: vec![
                "ListEntries".to_string(),
                "OpenFolder".to_string(),
                "CopySelection".to_string(),
                "MoveSelection".to_string(),
                "DeleteSelection".to_string(),
                "InspectItem".to_string(),
            ],
        };

        let default_sort = SortSpec {
            by: "name".to_string(),
            order: "asc".to_string(),
        };

        let left = Panel {
            panel_id: "left".to_string(),
            source_id: source_id.clone(),
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
            source_id: source_id.clone(),
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

        let mut sources = HashMap::new();
        sources.insert(source_id.clone(), source);

        let driver: Arc<dyn VfsDriver> = if std::env::var("FBRO_SPY").is_ok() {
            let inner = Arc::new(crate::vfs::local::LocalFsDriver);
            let sock_path = std::env::var("FBRO_SPY_SOCK")
                .unwrap_or_else(|_| "/tmp/fbro-spy.sock".to_string());
            Arc::new(crate::vfs::spy::SpyFsDriver::new(inner, sock_path))
        } else {
            Arc::new(crate::vfs::local::LocalFsDriver)
        };
        let mut drivers = HashMap::new();
        drivers.insert(source_id, driver);

        AppState {
            panels,
            sources,
            drivers,
            focus: "left".to_string(),
            platform,
            job_store: JobStore::new(),
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;
