use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    pub focus: String,
}

impl AppState {
    pub fn new_default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
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
            path: home.clone(),
            status: PanelStatus::Loading,
            sort: default_sort.clone(),
            filter: None,
            cursor: 0,
            entries: vec![],
            version: 0,
        };

        let right = Panel {
            panel_id: "right".to_string(),
            source_id: source_id.clone(),
            path: home.clone(),
            status: PanelStatus::Loading,
            sort: default_sort,
            filter: None,
            cursor: 0,
            entries: vec![],
            version: 0,
        };

        let mut panels = HashMap::new();
        panels.insert("left".to_string(), left);
        panels.insert("right".to_string(), right);

        let mut sources = HashMap::new();
        sources.insert(source_id, source);

        AppState {
            panels,
            sources,
            focus: "left".to_string(),
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;
