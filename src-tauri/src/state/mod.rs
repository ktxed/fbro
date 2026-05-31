pub mod settings;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::bookmarks::BookmarkEntry;
use crate::bro::ResolvedBro;
use crate::fileserver::ThumbCache;
use crate::jobs::search::SearchResultStore;
use crate::jobs::JobStore;
use crate::platform::{Platform, ResolvedOpenWithApp};
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
    /// Optional icon hint for the source bar (FE maps name → component).
    /// When `Some`, the FE renders the icon in place of the textual label.
    pub icon_name: Option<String>,
    /// Order key for the source bar. Lower = earlier. Filesystem volumes use
    /// the default `100`; non-volume virtual sources (e.g. procs) use a lower
    /// value to sort first.
    pub sort_priority: i32,
    /// Source is read-only at the BE level — disables write-capable jobs and
    /// surfaces a lock badge in the FE. Distinct from `shadow`, which is for
    /// transient per-panel overlays. Remote sources set this to `true`.
    pub read_only: bool,
    /// For `kind == "remote"` sources: stable id of the paired peer that
    /// owns this source. `None` for filesystem / virtual sources.
    pub peer_id: Option<String>,
}

// ─── Remote: discovery & pairing ──────────────────────────────────────────────

/// An mDNS-discovered fbro peer (or candidate). Volatile — never persisted.
/// `pubkey_fp` is populated when the TXT record carries `fp=...`; once a peer
/// is paired, its id is `"fbro:<pubkey_fp>"`, otherwise `"mdns:<host>:<port>"`.
#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub os: Option<String>,
    pub pubkey_fp: Option<String>,
    pub last_seen: std::time::SystemTime,
}

/// A peer we've successfully paired with. Persisted to
/// `~/.config/fbro/peers.json`. The fingerprint is the SHA256 of the peer's
/// Ed25519 public key, base64-encoded — used as the stable key for routing
/// remote sources and revoking trust.
#[derive(Clone, Debug)]
pub struct PairedPeer {
    pub fingerprint: String,
    pub pubkey: Vec<u8>,
    pub name: String,
    pub os: Option<String>,
    pub paired_at: std::time::SystemTime,
    pub last_seen: Option<std::time::SystemTime>,
    /// Last known SFTP endpoint for this peer, learned from mDNS or a
    /// successful connection. Used for manual retry when mDNS is quiet.
    pub last_host: Option<String>,
    pub last_sftp_port: Option<u16>,
    pub last_pair_port: Option<u16>,
    pub last_endpoint_seen_at: Option<std::time::SystemTime>,
}

/// In-process configuration controlling whether this fbro shares any of its
/// sources on the LAN and which peers are trusted. In v1, `enabled` and
/// `shared_source_ids` are volatile (reset on restart); only `paired_peers`
/// is persisted via `~/.config/fbro/peers.json`.
#[derive(Clone, Debug, Default)]
pub struct ShareConfig {
    pub enabled: bool,
    pub allow_ipv6: bool,
    pub shared_source_ids: HashSet<String>,
    pub shared_roots: Vec<SharedRoot>,
    pub paired_peers: HashMap<String, PairedPeer>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedRootOrigin {
    Source,
    Home,
    Bookmark,
}

impl SharedRootOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            SharedRootOrigin::Source => "source",
            SharedRootOrigin::Home => "home",
            SharedRootOrigin::Bookmark => "bookmark",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "home" => SharedRootOrigin::Home,
            "bookmark" => SharedRootOrigin::Bookmark,
            _ => SharedRootOrigin::Source,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedRoot {
    pub id: String,
    pub name: String,
    pub path: String,
    pub source_id: String,
    pub origin: SharedRootOrigin,
    pub icon_name: Option<String>,
}

pub fn share_root_id_for_path(origin: &SharedRootOrigin, path: &str) -> String {
    match origin {
        SharedRootOrigin::Source => path.to_string(),
        SharedRootOrigin::Home => {
            let hash = blake3::hash(format!("{}:{path}", origin.as_str()).as_bytes());
            format!("home-{}", &hash.to_hex()[..10])
        }
        SharedRootOrigin::Bookmark => {
            let hash = blake3::hash(format!("{}:{path}", origin.as_str()).as_bytes());
            let label = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(sanitize_share_id_part)
                .filter(|part| !part.is_empty())
                .unwrap_or_else(|| "bookmark".to_string());
            format!("bookmark-{label}-{}", &hash.to_hex()[..10])
        }
    }
}

fn sanitize_share_id_part(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

fn source_contains_path(source_root: &str, path: &str) -> bool {
    let source_root = Path::new(source_root);
    let path = Path::new(path);
    path == source_root || path.starts_with(source_root)
}

pub fn source_id_for_local_path(sources: &HashMap<String, Source>, path: &str) -> Option<String> {
    sources
        .values()
        .filter(|src| {
            src.kind == "filesystem"
                && src.mounted
                && !src.shadow
                && source_contains_path(&src.root_path, path)
        })
        .max_by_key(|src| src.root_path.len())
        .map(|src| src.id.clone())
}

fn fallback_source_id_for_home_candidates<'a, I>(candidates: I, home: &str) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str, &'a str)>,
{
    let candidates: Vec<(&str, &str, &str)> = candidates.into_iter().collect();
    candidates
        .iter()
        .filter(|(_, root_path, _)| source_contains_path(root_path, home))
        .max_by_key(|(_, root_path, _)| root_path.len())
        .map(|(id, _, _)| (*id).to_string())
        .or_else(|| {
            candidates
                .iter()
                .find(|(_, root_path, _)| *root_path == "/")
                .map(|(id, _, _)| (*id).to_string())
        })
        .or_else(|| {
            candidates
                .iter()
                .min_by(|a, b| {
                    a.2.to_lowercase()
                        .cmp(&b.2.to_lowercase())
                        .then_with(|| a.1.cmp(b.1))
                })
                .map(|(id, _, _)| (*id).to_string())
        })
}

pub fn shared_root_for_source(source: &Source) -> SharedRoot {
    SharedRoot {
        id: source.id.clone(),
        name: source.name.clone(),
        path: source.root_path.clone(),
        source_id: source.id.clone(),
        origin: SharedRootOrigin::Source,
        icon_name: source.icon_name.clone(),
    }
}

pub fn shared_root_for_path(
    sources: &HashMap<String, Source>,
    name: String,
    path: String,
    origin: SharedRootOrigin,
) -> Option<SharedRoot> {
    if !Path::new(&path).is_dir() {
        return None;
    }
    let source_id = source_id_for_local_path(sources, &path)?;
    let icon_name = sources.get(&source_id).and_then(|s| s.icon_name.clone());
    Some(SharedRoot {
        id: share_root_id_for_path(&origin, &path),
        name,
        path,
        source_id,
        origin,
        icon_name,
    })
}

pub fn share_suggestions(
    sources: &HashMap<String, Source>,
    bookmarks: &[BookmarkEntry],
    home: &str,
) -> Vec<SharedRoot> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for source in sources
        .values()
        .filter(|src| src.kind == "filesystem" && src.mounted && !src.shadow)
    {
        let root = shared_root_for_source(source);
        seen.insert(root.path.clone());
        out.push(root);
    }
    if let Some(root) = shared_root_for_path(
        sources,
        "Home".to_string(),
        home.to_string(),
        SharedRootOrigin::Home,
    ) {
        if seen.insert(root.path.clone()) {
            out.push(root);
        }
    }
    for bookmark in bookmarks {
        if let Some(root) = shared_root_for_path(
            sources,
            bookmark.name.clone(),
            bookmark.path.clone(),
            SharedRootOrigin::Bookmark,
        ) {
            if seen.insert(root.path.clone()) {
                out.push(root);
            }
        }
    }
    out.sort_by(|a, b| {
        a.origin
            .as_str()
            .cmp(b.origin.as_str())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

pub fn migrate_saved_shared_roots(
    sources: &HashMap<String, Source>,
    saved_source_ids: &HashSet<String>,
    saved_roots: Vec<SharedRoot>,
) -> Vec<SharedRoot> {
    if !saved_roots.is_empty() {
        return saved_roots
            .into_iter()
            .filter(|root| source_id_for_local_path(sources, &root.path).is_some())
            .collect();
    }
    saved_source_ids
        .iter()
        .filter_map(|id| sources.get(id))
        .filter(|src| src.kind == "filesystem" && src.mounted && !src.shadow)
        .map(shared_root_for_source)
        .collect()
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
    /// mDNS-discovered fbro peers on the LAN, keyed by peer id.
    /// Volatile — never persisted.
    pub discovered_peers: HashMap<String, DiscoveredPeer>,
    /// Sharing toggle, shared sources, and paired-peer trust list.
    pub share_config: ShareConfig,
    /// Lazily initialized Ed25519 identity for the embedded SSH server and
    /// outbound SFTP client. `None` until the first sharing / remote intent
    /// loads it from `~/.config/fbro/id_ed25519`.
    pub identity: Option<Arc<crate::remote::keys::Identity>>,
    /// Tokio handle for the running russh listener, when sharing is enabled.
    pub remote_server_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for the running mDNS discovery task. Spawned once at startup;
    /// `None` only on early boot before wiring.
    pub discovery_handle: Option<crate::remote::discovery::DiscoveryHandle>,
    /// Tokio handle for the pairing TCP listener (port 7880). Started by
    /// `EnableSharing` so peers can initiate handshakes against us.
    pub pair_listener_handle: Option<tokio::task::JoinHandle<()>>,
    /// In-flight pairing handshakes, keyed by handshake id. The
    /// oneshot::Sender feeds the user's Confirm/Cancel into the protocol
    /// driver task; the task removes itself when it completes (success,
    /// rejection, or timeout).
    pub active_handshakes: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    /// Requests from short-lived CLI invocations. They are drained by the
    /// active GUI connection so scans/watchers stay owned by that connection.
    pub pending_cli_opens: Vec<CliOpenRequest>,
}

#[derive(Clone, Debug)]
pub struct CliOpenRequest {
    pub path: String,
    pub cwd: String,
    pub create_tab: bool,
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
    pub fn fallback_source_id_for_home(&self) -> Option<String> {
        let home = self.platform.home_dir();
        fallback_source_id_for_home_candidates(
            self.sources.iter().filter_map(|(id, source)| {
                (source.kind == "filesystem"
                    && source.mounted
                    && !source.shadow
                    && self.drivers.contains_key(id))
                .then_some((id.as_str(), source.root_path.as_str(), source.name.as_str()))
            }),
            &home,
        )
    }

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
                icon_name: None,
                sort_priority: 100,
                read_only: false,
                peer_id: None,
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
                // In hierarchical mode directory entries are intra-driver tree
                // nodes — advertise InternalNavigation so open_entry does not
                // try to hand off to the local FS driver.
                supports: {
                    let mut caps = vec!["ListEntries".to_string(), "DeleteSelection".to_string()];
                    if loaded_settings.general.proc_tree_mode {
                        caps.push(crate::ws::intents::CAP_INTERNAL_NAV.to_string());
                    }
                    caps
                },
                root_path: String::new(),
                mounted: true,
                device_path: None,
                shadow: false,
                icon_name: Some("activity".to_string()),
                sort_priority: 0,
                read_only: false,
                peer_id: None,
            },
        );
        drivers.insert(
            "procs".to_string(),
            Arc::new(crate::vfs::procs::ProcsDriver {
                hierarchical: loaded_settings.general.proc_tree_mode,
            }),
        );

        let share_config = {
            // Best-effort load from `~/.config/fbro/peers.json`. A missing
            // or malformed file yields an empty map — pairing still works,
            // the user just has nobody trusted yet.
            let paired_peers =
                crate::remote::pairing::load_peers(&crate::remote::pairing::default_peers_path())
                    .unwrap_or_default();
            // Restore the user's last sharing toggle + source picks. Same
            // tolerance: missing/corrupt file → defaults (off, empty set).
            let saved =
                crate::remote::share_state::load(&crate::remote::share_state::default_share_path())
                    .unwrap_or_default();
            let shared_roots =
                migrate_saved_shared_roots(&sources, &saved.shared_source_ids, saved.shared_roots);
            let shared_source_ids = shared_roots
                .iter()
                .filter(|root| matches!(root.origin, SharedRootOrigin::Source))
                .map(|root| root.source_id.clone())
                .collect();
            ShareConfig {
                enabled: saved.enabled,
                allow_ipv6: saved.allow_ipv6,
                shared_source_ids,
                shared_roots,
                paired_peers,
            }
        };

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
            discovered_peers: HashMap::new(),
            share_config,
            identity: None,
            remote_server_handle: None,
            discovery_handle: None,
            pair_listener_handle: None,
            active_handshakes: HashMap::new(),
            pending_cli_opens: Vec::new(),
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
        // Only consider filesystem-kind sources for removal. Virtual sources
        // (procs, bookmarks, history, archives, search) have their own
        // lifecycle and are not in the volume watcher's purview — leaving
        // them in `old_ids` would let `rebuild_sources` silently delete them
        // any time volumes change.
        let old_ids: HashSet<String> = self
            .sources
            .iter()
            .filter(|(_, src)| !src.shadow && src.kind == "filesystem")
            .map(|(id, _)| id.clone())
            .collect();

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
                    icon_name: None,
                    sort_priority: 100,
                    read_only: false,
                    peer_id: None,
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
                        let resolved =
                            pick_source(source_id).unwrap_or_else(|| panel.source_id.clone());
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

#[cfg(test)]
mod tests {
    use super::fallback_source_id_for_home_candidates;

    #[test]
    fn fallback_source_prefers_home_owner() {
        let candidates = [
            ("vol-root", "/", "/"),
            ("vol-home", "/home/user", "home"),
            ("vol-media", "/run/media/user/ssd", "ssd"),
        ];

        assert_eq!(
            fallback_source_id_for_home_candidates(candidates, "/home/user/projects").as_deref(),
            Some("vol-home")
        );
    }

    #[test]
    fn fallback_source_uses_root_when_home_owner_missing() {
        let candidates = [
            ("vol-root", "/", "/"),
            ("vol-media", "/run/media/user/ssd", "ssd"),
        ];

        assert_eq!(
            fallback_source_id_for_home_candidates(candidates, "/home/user").as_deref(),
            Some("vol-root")
        );
    }

    #[test]
    fn fallback_source_uses_stable_first_available_without_home_or_root() {
        let candidates = [("vol-z", "/mnt/z", "zeta"), ("vol-a", "/mnt/a", "alpha")];

        assert_eq!(
            fallback_source_id_for_home_candidates(candidates, "/home/user").as_deref(),
            Some("vol-a")
        );
    }

    #[test]
    fn fallback_source_returns_none_without_candidates() {
        let candidates: [(&str, &str, &str); 0] = [];
        assert_eq!(
            fallback_source_id_for_home_candidates(candidates, "/home/user"),
            None
        );
    }
}
