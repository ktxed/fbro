//! Embedded `russh` server + SFTP subsystem for serving fbro sources to
//! paired peers on the LAN.
//!
//! Wire model:
//!  - russh listener on `:7879` (configurable).
//!  - Pubkey auth only: the incoming key's raw 32-byte Ed25519 bytes are
//!    hashed (SHA256 + URL-safe base64) and looked up in
//!    `share_config.paired_peers`.
//!  - On accept, a single subsystem is supported: `sftp`. The SFTP
//!    filesystem is rooted at `/` and shows one directory per id in
//!    `share_config.shared_source_ids`.
//!  - Read-only in v1: write paths return `SSH_FX_OP_UNSUPPORTED`.
//!  - Custom extension `fbro@anthropic.com/list-sources` returns a JSON
//!    array of `{id, name, icon, read_only}` so the fbro-aware client can
//!    render proper labels/icons in the volume bar.
//!
//! Pairing is handled by a separate TCP listener in `pair_transport.rs`
//! because the russh version we're on doesn't expose custom-channel-type
//! open callbacks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use russh::keys::ssh_key::public::KeyData;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File as SftpFile, FileAttributes, Handle, Name, OpenFlags, Packet, Status,
    StatusCode, Version,
};
use serde::Serialize;
use tokio::sync::{watch, Mutex};

use crate::jobs::{
    AppJob, FileOpProgress, JobKind, JobState, OutgoingTransferJobMeta, RemoteManifestJobMeta,
};
use crate::remote::keys::{fingerprint_of, Identity};
use crate::state::SharedState;
use crate::vfs::{hash_local_file_blake3, VfsDriver, VfsError, VfsKind};
use crate::ws::protocol::{
    now, FileOpFileSchema, FileOpProgressSchema, JobStartSchema, ServerMessage,
};

pub const DEFAULT_PORT: u16 = 7879;
const SSH_CHANNEL_WINDOW_SIZE: u32 = 16 * 1024 * 1024;
const FBRO_INTEGRITY_ALGORITHM: &str = "blake3";
const MANIFEST_HASH_CHUNK_SIZE: usize = 1024 * 1024;
const SYNC_REPORT_NAME: &str = "fbro-sync-report.txt";

/// Hook the discovery module passes in so the server can emit
/// `RemoteSessionStarted` / `RemoteSessionEnded` to GUI clients.
pub trait SessionEvents: Send + Sync {
    fn started(&self, fingerprint: &str);
    fn ended(&self, fingerprint: &str);
}

/// No-op `SessionEvents` for tests / when the WS server isn't wired yet.
pub struct NoopSessionEvents;
impl SessionEvents for NoopSessionEvents {
    fn started(&self, _: &str) {}
    fn ended(&self, _: &str) {}
}

/// Production session-events impl — sends `RemoteSession*` messages to all
/// `gui_senders` registered on the shared state.
pub struct GuiSessionEvents {
    events: tokio::sync::mpsc::UnboundedSender<(String, bool)>,
}

impl GuiSessionEvents {
    pub fn new(state: SharedState) -> Self {
        let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel::<(String, bool)>();
        // This worker belongs to the application runtime. Auth/drop callbacks
        // enqueue in order, so a short session cannot end before it starts.
        tokio::spawn(async move {
            while let Some((fingerprint, started)) = receiver.recv().await {
                emit_session(&state, fingerprint, started).await;
            }
        });
        Self { events }
    }
}

impl SessionEvents for GuiSessionEvents {
    fn started(&self, fingerprint: &str) {
        let _ = self.events.send((fingerprint.to_string(), true));
    }
    fn ended(&self, fingerprint: &str) {
        let _ = self.events.send((fingerprint.to_string(), false));
    }
}

/// Production `StatusBroadcaster` for the SFTP driver — sends
/// `RemoteSourceStatus` messages to every connected GUI client.
pub struct GuiStatusBroadcaster {
    events: tokio::sync::mpsc::UnboundedSender<ServerMessage>,
}

impl GuiStatusBroadcaster {
    pub fn new(state: SharedState, source_id: String, generation: u64) -> Self {
        let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
        // The driver has its own runtime, which is torn down when unmounted.
        // Process its ordered status events on the application runtime instead.
        tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                if let ServerMessage::RemoteSourceStatus {
                    source_id: message_source, status, ..
                } = &message {
                    if message_source != &source_id {
                        continue;
                    }
                    if !crate::notifications::remote::source_status(
                        &state, &source_id, generation, status,
                    ).await {
                        continue;
                    }
                }
                let senders = state.lock().await.gui_senders.clone();
                for tx in senders {
                    let _ = tx.try_send(message.clone());
                }
            }
        });
        Self { events }
    }
}

impl crate::vfs::sftp::StatusBroadcaster for GuiStatusBroadcaster {
    fn emit(
        &self,
        source_id: &str,
        status: &str,
        message: Option<String>,
        next_retry_at: Option<u64>,
    ) {
        let msg = ServerMessage::RemoteSourceStatus {
            timestamp: now(),
            source_id: source_id.to_string(),
            status: status.to_string(),
            message,
            next_retry_at,
        };
        let _ = self.events.send(msg);
    }
}

async fn emit_session(state: &SharedState, fingerprint: String, started: bool) {
    crate::notifications::remote::incoming_session(state, &fingerprint, started).await;
    let senders = state.lock().await.gui_senders.clone();
    // source_id is empty here — the FE tracks active sessions per-peer,
    // not per-source. Future: emit one event per opened source.
    let msg = if started {
        ServerMessage::RemoteSessionStarted {
            timestamp: now(),
            fingerprint,
            source_id: String::new(),
        }
    } else {
        ServerMessage::RemoteSessionEnded {
            timestamp: now(),
            fingerprint,
            source_id: String::new(),
        }
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

const OUTGOING_PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
const OUTGOING_COMPLETE_IDLE: Duration = Duration::from_millis(1_500);
const MANIFEST_STREAM_BUF_SIZE: usize = 8 * 1024 * 1024;

fn progress_schema(progress: &FileOpProgress) -> FileOpProgressSchema {
    FileOpProgressSchema {
        total_files: progress.total_files,
        completed_files: progress.completed_files,
        current_file: progress.current_file.clone(),
        status_text: progress.status_text.clone(),
        current_file_bytes_total: progress.current_file_bytes_total,
        current_file_bytes_copied: progress.current_file_bytes_copied,
        total_bytes_total: progress.total_bytes_total,
        total_bytes_copied: progress.total_bytes_copied,
        current_file_percent: percent(
            progress.current_file_bytes_copied,
            progress.current_file_bytes_total,
        ),
        overall_percent: percent(progress.total_bytes_copied, progress.total_bytes_total),
    }
}

fn percent(done: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(((done.saturating_mul(100) / total).min(100)) as u8)
}

/// Spawn the russh listener. Returns a `JoinHandle` that resolves when the
/// listener exits (currently never — it's a long-lived task).
pub async fn spawn(
    state: SharedState,
    identity: Arc<Identity>,
    port: u16,
    events: Arc<dyn SessionEvents>,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let signing_seed = identity.signing.to_bytes();
    let private = russh::keys::ssh_key::private::Ed25519PrivateKey::from_bytes(&signing_seed);
    let public = russh::keys::ssh_key::public::Ed25519PublicKey::from(&private);
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair { public, private };
    let host_key: russh::keys::PrivateKey = keypair.into();

    let config = russh::server::Config {
        auth_rejection_time: std::time::Duration::from_secs(3),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: vec![host_key],
        window_size: SSH_CHANNEL_WINDOW_SIZE,
        nodelay: true,
        ..Default::default()
    };

    let server = FbroServer {
        state: state.clone(),
        events,
    };

    let mut server_owned = server;
    let handle = tokio::spawn(async move {
        if let Err(e) = server_owned
            .run_on_address(Arc::new(config), ("0.0.0.0", port))
            .await
        {
            eprintln!("[remote-server] listener exited: {e}");
        }
    });
    Ok(handle)
}

// ─── russh server / per-connection handler ────────────────────────────────────

#[derive(Clone)]
struct FbroServer {
    state: SharedState,
    events: Arc<dyn SessionEvents>,
}

impl russh::server::Server for FbroServer {
    type Handler = ConnectionHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        ConnectionHandler {
            state: self.state.clone(),
            events: self.events.clone(),
            peer_fp: None,
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct ConnectionHandler {
    state: SharedState,
    events: Arc<dyn SessionEvents>,
    /// Set after successful pubkey auth — the fingerprint we matched against
    /// `paired_peers`. Used both for SFTP authorization and for emitting
    /// `RemoteSession*` events.
    peer_fp: Option<String>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl Drop for ConnectionHandler {
    fn drop(&mut self) {
        if let Some(fp) = self.peer_fp.clone() {
            self.events.ended(&fp);
        }
    }
}

impl russh::server::Handler for ConnectionHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        // Pubkey-only — passwords are never accepted on the fbro listener.
        Ok(Auth::reject())
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        // Only Ed25519 is supported — pairing always uses Ed25519, and
        // accepting other algorithms widens the attack surface.
        let raw = match public_key.key_data() {
            KeyData::Ed25519(ed) => ed.0,
            _ => return Ok(Auth::reject()),
        };
        let fp = fingerprint_of(&raw);
        let allowed = {
            let guard = self.state.lock().await;
            guard.share_config.paired_peers.contains_key(&fp)
        };
        if allowed {
            self.peer_fp = Some(fp.clone());
            // Update `last_seen` for the peer.
            {
                let mut guard = self.state.lock().await;
                if let Some(peer) = guard.share_config.paired_peers.get_mut(&fp) {
                    peer.last_seen = Some(SystemTime::now());
                }
            }
            self.events.started(&fp);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        {
            let mut guard = self.channels.lock().await;
            guard.insert(channel.id(), channel);
        }
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        // peer_fp must be set — auth_publickey ran first and refused
        // anything else, so this is just defence-in-depth.
        let Some(peer_fp) = self.peer_fp.clone() else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        let channel = {
            let mut guard = self.channels.lock().await;
            match guard.remove(&channel_id) {
                Some(ch) => ch,
                None => {
                    session.channel_failure(channel_id)?;
                    return Ok(());
                }
            }
        };
        session.channel_success(channel_id)?;
        let sftp = SftpHandler::new(self.state.clone(), peer_fp);
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), sftp).await;
        });
        Ok(())
    }
}

// ─── SFTP subsystem handler ───────────────────────────────────────────────────

struct SftpHandler {
    state: SharedState,
    /// Fingerprint of the paired peer driving this subsystem. Reserved for
    /// future per-peer ACL / audit-logging; not load-bearing today.
    #[allow(dead_code)]
    peer_fp: String,
    handles: HashMap<String, HandleState>,
    next_handle: u64,
    version: Option<u32>,
    transfers: HashMap<String, OutgoingTransfer>,
    session_transfer_key: Option<String>,
    manifest_jobs: HashMap<String, Arc<Mutex<ManifestJobState>>>,
}

enum HandleState {
    /// Pre-materialized directory listing. The driver may be expensive to
    /// call (network, large archives) so we snapshot once at `opendir` time
    /// and serve `readdir` from memory.
    Dir {
        source_id: Option<String>,
        path_in_source: String,
        entries: Vec<DirEntry>,
        cursor: usize,
    },
    Control,
    ControlData {
        data: Vec<u8>,
    },
    File {
        source_id: String,
        path_in_source: String,
        display_path: String,
        transfer_key: String,
        /// Cached size for `fstat`. None if the driver doesn't know.
        size: Option<u64>,
        completed: bool,
    },
}

struct OutgoingTransfer {
    job_id: String,
    cancel_rx: watch::Receiver<bool>,
    progress: FileOpProgress,
    files: Vec<FileOpFileSchema>,
    file_indexes: HashMap<String, usize>,
    completed_files: HashSet<String>,
    file_ranges: HashMap<String, Vec<(u64, u64)>>,
    file_digests: HashMap<String, OrderedTransferDigest>,
    last_emit: Instant,
    completion_generation: u64,
}

#[derive(Default)]
struct ManifestJobState {
    done: bool,
    cancelled: bool,
    error: Option<String>,
    processed: usize,
    current_path: Option<String>,
    current_file_bytes_total: u64,
    current_file_bytes_hashed: u64,
    updated_at: u64,
    entries: Vec<ManifestEntryResponse>,
    errors: Vec<ManifestErrorResponse>,
    visible_job_id: String,
    visible_cancel_rx: Option<watch::Receiver<bool>>,
    last_emit: Option<Instant>,
}

#[derive(Clone, Serialize)]
struct ManifestEntryResponse {
    path: String,
    size: u64,
    hash: String,
}

#[derive(Clone, Serialize)]
struct ManifestErrorResponse {
    path: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestStartResponse {
    job_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestStatusResponse {
    done: bool,
    cancelled: bool,
    processed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_path: Option<String>,
    current_file_bytes_total: u64,
    current_file_bytes_hashed: u64,
    updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<ManifestEntryResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<ManifestErrorResponse>>,
}

struct OrderedTransferDigest {
    hasher: blake3::Hasher,
    pending: BTreeMap<u64, Vec<u8>>,
    next_offset: u64,
    size: Option<u64>,
    finalized: Option<String>,
}

#[derive(Serialize)]
struct DigestResponse {
    algorithm: &'static str,
    size: u64,
    hash: String,
    complete: bool,
}

#[derive(Serialize)]
struct SourceMetadataItem {
    id: String,
    name: String,
    icon: Option<String>,
    read_only: bool,
    supports_thumbnails: bool,
    supports_manifest: bool,
}

#[derive(Clone)]
struct DirEntry {
    name: String,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<u32>,
}

impl SftpHandler {
    fn new(state: SharedState, peer_fp: String) -> Self {
        Self {
            state,
            peer_fp,
            handles: HashMap::new(),
            next_handle: 0,
            version: None,
            transfers: HashMap::new(),
            session_transfer_key: None,
            manifest_jobs: HashMap::new(),
        }
    }

    fn fresh_handle(&mut self) -> String {
        self.next_handle = self.next_handle.wrapping_add(1);
        format!("h{}", self.next_handle)
    }

    async fn ensure_transfer(&mut self, key: &str) -> Option<&mut OutgoingTransfer> {
        if !self.transfers.contains_key(key) {
            let transfer = start_outgoing_transfer(&self.state, &self.peer_fp, key).await?;
            self.transfers.insert(key.to_string(), transfer);
        }
        self.transfers.get_mut(key)
    }

    async fn note_file_opened(&mut self, transfer_key: &str, path: &str, size: Option<u64>) {
        if self.transfer_cancelled_for(transfer_key).await {
            return;
        }
        let state = self.state.clone();
        let Some(transfer) = self.ensure_transfer(transfer_key).await else {
            return;
        };
        let reset_digest = transfer
            .file_digests
            .get(path)
            .map(OrderedTransferDigest::is_complete)
            .unwrap_or(false);
        if reset_digest {
            transfer
                .file_digests
                .insert(path.to_string(), OrderedTransferDigest::new(size));
            transfer.file_ranges.remove(path);
            transfer.completed_files.remove(path);
        } else {
            transfer
                .file_digests
                .entry(path.to_string())
                .or_insert_with(|| OrderedTransferDigest::new(size));
        }
        transfer.completion_generation = transfer.completion_generation.wrapping_add(1);
        if !transfer.file_indexes.contains_key(path) {
            let file = FileOpFileSchema {
                name: path
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(path)
                    .to_string(),
                size,
                kind: "file".to_string(),
                source_path: Some(path.to_string()),
                dest_name: None,
            };
            transfer
                .file_indexes
                .insert(path.to_string(), transfer.files.len());
            transfer.files.push(file);
            transfer.progress.total_files = transfer.files.len();
            transfer.progress.total_bytes_total = transfer
                .progress
                .total_bytes_total
                .saturating_add(size.unwrap_or(0));
        }
        transfer.progress.current_file = Some(path.to_string());
        transfer.progress.current_file_bytes_total = size.unwrap_or(0);
        transfer.progress.current_file_bytes_copied = covered_len(
            transfer
                .file_ranges
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        transfer.progress.status_text = Some("Sending to remote fbro".to_string());
        emit_outgoing_progress(&state, transfer, true).await;
    }

    async fn note_file_read(
        &mut self,
        transfer_key: &str,
        path: &str,
        offset: u64,
        data: &[u8],
        len: usize,
        size: Option<u64>,
    ) {
        if len == 0 {
            return;
        }
        if self.transfer_cancelled_for(transfer_key).await {
            return;
        }
        let state = self.state.clone();
        let Some(transfer) = self.ensure_transfer(transfer_key).await else {
            return;
        };
        transfer
            .file_digests
            .entry(path.to_string())
            .or_insert_with(|| OrderedTransferDigest::new(size))
            .add_chunk(offset, data);
        let end = offset.saturating_add(len as u64);
        let added = add_covered_range(
            transfer.file_ranges.entry(path.to_string()).or_default(),
            offset,
            end,
        );
        if added == 0 {
            return;
        }
        transfer.completion_generation = transfer.completion_generation.wrapping_add(1);
        transfer.progress.total_bytes_copied =
            transfer.progress.total_bytes_copied.saturating_add(added);
        transfer.progress.current_file = Some(path.to_string());
        transfer.progress.current_file_bytes_total = size.unwrap_or(0);
        transfer.progress.current_file_bytes_copied = covered_len(
            transfer
                .file_ranges
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        transfer.progress.status_text = Some("Sending to remote fbro".to_string());
        emit_outgoing_progress(&state, transfer, false).await;
    }

    async fn note_file_closed(
        &mut self,
        transfer_key: &str,
        path: &str,
        size: Option<u64>,
        completed: bool,
    ) {
        if !completed || self.transfer_cancelled_for(transfer_key).await {
            return;
        }
        let state = self.state.clone();
        let completion_to_schedule = {
            let Some(transfer) = self.transfers.get_mut(transfer_key) else {
                return;
            };
            let copied = covered_len(
                transfer
                    .file_ranges
                    .get(path)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
            if size.map(|total| copied >= total).unwrap_or(copied > 0)
                && transfer.completed_files.insert(path.to_string())
            {
                if let Some(digest) = transfer.file_digests.get_mut(path) {
                    digest.try_finalize();
                }
                transfer.progress.completed_files =
                    transfer.progress.completed_files.saturating_add(1);
            }
            transfer.progress.current_file = Some(path.to_string());
            transfer.progress.current_file_bytes_total = size.unwrap_or(0);
            transfer.progress.current_file_bytes_copied = copied;
            emit_outgoing_progress(&state, transfer, true).await;

            let all_known_files_done = transfer.progress.total_files > 0
                && transfer.progress.completed_files >= transfer.progress.total_files
                && (transfer.progress.total_bytes_total == 0
                    || transfer.progress.total_bytes_copied >= transfer.progress.total_bytes_total);
            if all_known_files_done {
                transfer.completion_generation = transfer.completion_generation.wrapping_add(1);
                Some((transfer.job_id.clone(), transfer.completion_generation))
            } else {
                None
            }
        };

        if let Some((job_id, generation)) = completion_to_schedule {
            schedule_outgoing_completion(state, job_id, generation);
        }
    }

    async fn transfer_cancelled_for(&self, transfer_key: &str) -> bool {
        if self
            .state
            .lock()
            .await
            .cancelled_remote_transfers
            .contains(&(self.peer_fp.clone(), transfer_key.to_string()))
        {
            return true;
        }
        self.transfers
            .get(transfer_key)
            .map(|transfer| *transfer.cancel_rx.borrow())
            .unwrap_or(false)
    }

    fn integrity_result_json(
        &self,
        job_id: &str,
        encoded_path: &str,
    ) -> Result<Vec<u8>, StatusCode> {
        let path_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| StatusCode::BadMessage)?;
        let path = String::from_utf8(path_bytes).map_err(|_| StatusCode::BadMessage)?;
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let transfer_key = format!("client:{job_id}");
        let Some(transfer) = self.transfers.get(&transfer_key) else {
            return Err(StatusCode::NoSuchFile);
        };
        let Some(digest) = transfer.file_digests.get(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        let response = DigestResponse {
            algorithm: FBRO_INTEGRITY_ALGORITHM,
            size: digest.size.unwrap_or(digest.next_offset),
            hash: digest.finalized.clone().unwrap_or_default(),
            complete: digest.is_complete(),
        };
        serde_json::to_vec(&response).map_err(|_| StatusCode::Failure)
    }

    async fn full_file_digest_json(&self, encoded_path: &str) -> Result<Vec<u8>, StatusCode> {
        let path_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| StatusCode::BadMessage)?;
        let path = String::from_utf8(path_bytes).map_err(|_| StatusCode::BadMessage)?;
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let (source_id, rest) = parse_path(&path);
        let Some(sid) = source_id else {
            return Err(StatusCode::NoSuchFile);
        };
        let Some((driver, root, _, _, _)) = resolve_source(&self.state, &sid).await else {
            return Err(StatusCode::NoSuchFile);
        };
        let driver_path = compose_driver_path(&root, &rest);
        let size = driver
            .file_size(&driver_path)
            .ok_or(StatusCode::NoSuchFile)?;
        if let Some(key) = self.session_transfer_key.as_deref() {
            if self.transfer_cancelled_for(key).await {
                return Err(StatusCode::PermissionDenied);
            }
        }
        let cancel_rx = self
            .session_transfer_key
            .as_deref()
            .and_then(|key| self.transfers.get(key))
            .map(|transfer| transfer.cancel_rx.clone());
        let digest = tokio::task::spawn_blocking(move || {
            let mut hasher = blake3::Hasher::new();
            let mut offset = 0u64;
            let chunk_size = 1024 * 1024usize;
            while offset < size {
                if cancel_rx.as_ref().is_some_and(|rx| *rx.borrow()) {
                    return Err(VfsError::Cancelled);
                }
                let remaining = size.saturating_sub(offset).min(chunk_size as u64) as usize;
                let chunk = driver.read_chunk(&driver_path, offset, remaining)?;
                if chunk.is_empty() {
                    break;
                }
                offset = offset.saturating_add(chunk.len() as u64);
                hasher.update(&chunk);
            }
            if cancel_rx.as_ref().is_some_and(|rx| *rx.borrow()) {
                return Err(VfsError::Cancelled);
            }
            if offset != size {
                return Err(VfsError::IOError(format!(
                    "digest read ended at {offset} of {size} bytes"
                )));
            }
            Ok::<String, VfsError>(hasher.finalize().to_hex().to_string())
        })
        .await
        .map_err(|_| StatusCode::Failure)?
        .map_err(vfs_err_to_status)?;
        let response = DigestResponse {
            algorithm: FBRO_INTEGRITY_ALGORITHM,
            size,
            hash: digest,
            complete: true,
        };
        serde_json::to_vec(&response).map_err(|_| StatusCode::Failure)
    }

    async fn start_manifest_json(&mut self, encoded_path: &str) -> Result<Vec<u8>, StatusCode> {
        let path_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| StatusCode::BadMessage)?;
        let path = String::from_utf8(path_bytes).map_err(|_| StatusCode::BadMessage)?;
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let (source_id, rest) = parse_path(&path);
        let Some(sid) = source_id else {
            return Err(StatusCode::NoSuchFile);
        };
        let Some((driver, root, _, _, _)) = resolve_source(&self.state, &sid).await else {
            return Err(StatusCode::NoSuchFile);
        };
        let root_path = compose_driver_path(&root, &rest);
        if !driver.exists(&root_path) {
            return Err(StatusCode::NoSuchFile);
        }

        let job_id = format!("manifest-{}", self.fresh_handle());
        let (visible_job_id, visible_cancel_rx) = start_remote_manifest_job(
            &self.state,
            &self.peer_fp,
            &root_path,
            &format!("/{sid}/{rest}").trim_end_matches('/').to_string(),
        )
        .await;
        eprintln!(
            "[remote-manifest] start control_job={job_id} ui_job={visible_job_id} root={root_path}"
        );
        let job = Arc::new(Mutex::new(ManifestJobState {
            updated_at: unix_millis_now(),
            visible_job_id,
            visible_cancel_rx: Some(visible_cancel_rx),
            last_emit: Some(Instant::now() - OUTGOING_PROGRESS_INTERVAL),
            ..Default::default()
        }));
        self.manifest_jobs.insert(job_id.clone(), Arc::clone(&job));

        let state = self.state.clone();
        let control_job_id = job_id.clone();
        tokio::task::spawn_blocking(move || {
            let result =
                build_manifest_on_remote(&state, &control_job_id, &*driver, &root_path, &job);
            let (visible_job_id, terminal_event, terminal_error, processed, error_count) = {
                let mut guard = job.blocking_lock();
                match result {
                    Ok(()) => {}
                    Err(error) if error == "Cancelled" => {
                        guard.cancelled = true;
                    }
                    Err(error) => {
                        guard.error = Some(error);
                    }
                }
                guard.done = true;
                guard.current_path = None;
                guard.current_file_bytes_total = 0;
                guard.current_file_bytes_hashed = 0;
                guard.updated_at = unix_millis_now();
                let event = if guard.cancelled {
                    "Cancelled"
                } else if guard.error.is_some() {
                    "Failed"
                } else {
                    "Completed"
                };
                (
                    guard.visible_job_id.clone(),
                    event.to_string(),
                    guard.error.clone(),
                    guard.processed,
                    guard.errors.len(),
                )
            };
            eprintln!(
                "[remote-manifest] finish control_job={control_job_id} ui_job={visible_job_id} event={terminal_event} processed={processed} errors={error_count}"
            );
            emit_remote_manifest_terminal_sync(
                &state,
                &visible_job_id,
                &terminal_event,
                terminal_error,
            );
        });

        serde_json::to_vec(&ManifestStartResponse { job_id }).map_err(|_| StatusCode::Failure)
    }

    async fn manifest_status_json(&mut self, job_id: &str) -> Result<Vec<u8>, StatusCode> {
        let Some(job) = self.manifest_jobs.get(job_id).cloned() else {
            return Err(StatusCode::NoSuchFile);
        };
        let guard = job.lock().await;
        let done = guard.done;
        let response = if done {
            ManifestStatusResponse {
                done: true,
                cancelled: guard.cancelled,
                processed: guard.processed,
                current_path: guard.current_path.clone(),
                current_file_bytes_total: guard.current_file_bytes_total,
                current_file_bytes_hashed: guard.current_file_bytes_hashed,
                updated_at: guard.updated_at,
                error: guard.error.clone(),
                entries: Some(guard.entries.clone()),
                errors: Some(guard.errors.clone()),
            }
        } else {
            ManifestStatusResponse {
                done: false,
                cancelled: guard.cancelled,
                processed: guard.processed,
                current_path: guard.current_path.clone(),
                current_file_bytes_total: guard.current_file_bytes_total,
                current_file_bytes_hashed: guard.current_file_bytes_hashed,
                updated_at: guard.updated_at,
                error: None,
                entries: None,
                errors: None,
            }
        };
        drop(guard);
        if done {
            self.manifest_jobs.remove(job_id);
        }
        serde_json::to_vec(&response).map_err(|_| StatusCode::Failure)
    }

    async fn manifest_cancel_json(&mut self, job_id: &str) -> Result<Vec<u8>, StatusCode> {
        let Some(job) = self.manifest_jobs.get(job_id).cloned() else {
            return Err(StatusCode::NoSuchFile);
        };
        let mut guard = job.lock().await;
        guard.cancelled = true;
        guard.updated_at = unix_millis_now();
        let visible_job_id = guard.visible_job_id.clone();
        let response = ManifestStatusResponse {
            done: guard.done,
            cancelled: true,
            processed: guard.processed,
            current_path: guard.current_path.clone(),
            current_file_bytes_total: guard.current_file_bytes_total,
            current_file_bytes_hashed: guard.current_file_bytes_hashed,
            updated_at: guard.updated_at,
            error: guard.error.clone(),
            entries: None,
            errors: None,
        };
        drop(guard);
        self.manifest_jobs.remove(job_id);
        eprintln!("[remote-manifest] cancel control_job={job_id} ui_job={visible_job_id}");
        emit_remote_manifest_terminal(&self.state, &visible_job_id, "Cancelled", None).await;
        serde_json::to_vec(&response).map_err(|_| StatusCode::Failure)
    }

    async fn sources_metadata_json(&self) -> Result<Vec<u8>, StatusCode> {
        let guard = self.state.lock().await;
        let stream_thumbnails = guard.settings.general.stream_thumbnails;
        let mut items: Vec<SourceMetadataItem> = Vec::new();
        for root in guard.share_config.shared_roots.iter() {
            let Some(driver) = guard.drivers.get(&root.source_id) else {
                continue;
            };
            if !share_root_available(&guard, root) {
                continue;
            }
            items.push(SourceMetadataItem {
                id: root.id.clone(),
                name: root.name.clone(),
                icon: root.icon_name.clone(),
                read_only: true,
                supports_thumbnails: stream_thumbnails && driver.supports_thumbnails(),
                supports_manifest: true,
            });
        }
        serde_json::to_vec(&items).map_err(|_| StatusCode::Failure)
    }

    async fn thumbnail_bytes(&self, size: u32, encoded_path: &str) -> Result<Vec<u8>, StatusCode> {
        let path_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| StatusCode::BadMessage)?;
        let path = String::from_utf8(path_bytes).map_err(|_| StatusCode::BadMessage)?;
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let stream_thumbnails = {
            let guard = self.state.lock().await;
            guard.settings.general.stream_thumbnails
        };
        if !stream_thumbnails {
            return Err(StatusCode::OpUnsupported);
        }
        let (source_id, rest) = parse_path(&path);
        let Some(sid) = source_id else {
            return Err(StatusCode::NoSuchFile);
        };
        let Some((driver, root, _, _, _)) = resolve_source(&self.state, &sid).await else {
            return Err(StatusCode::NoSuchFile);
        };
        if !driver.supports_thumbnails() {
            return Err(StatusCode::OpUnsupported);
        }
        let driver_path = compose_driver_path(&root, &rest);
        if !driver.exists(&driver_path) || driver.file_size(&driver_path).is_none() {
            return Err(StatusCode::NoSuchFile);
        }
        let platform = {
            let guard = self.state.lock().await;
            guard.platform.clone()
        };
        tokio::task::spawn_blocking(move || {
            platform
                .get_thumbnail(&driver_path, size.clamp(32, 512))
                .map(|(bytes, _)| bytes)
                .ok_or(StatusCode::NoSuchFile)
        })
        .await
        .map_err(|_| StatusCode::Failure)?
    }
}

impl OrderedTransferDigest {
    fn new(size: Option<u64>) -> Self {
        Self {
            hasher: blake3::Hasher::new(),
            pending: BTreeMap::new(),
            next_offset: 0,
            size,
            finalized: None,
        }
    }

    fn add_chunk(&mut self, offset: u64, data: &[u8]) {
        if self.finalized.is_some() || data.is_empty() {
            return;
        }

        let mut start = offset;
        let mut bytes = data;
        if start < self.next_offset {
            let skip = (self.next_offset - start) as usize;
            if skip >= bytes.len() {
                return;
            }
            bytes = &bytes[skip..];
            start = self.next_offset;
        }
        if self.pending.contains_key(&start) {
            return;
        }
        self.pending.insert(start, bytes.to_vec());
        self.drain_contiguous();
        self.try_finalize();
    }

    fn drain_contiguous(&mut self) {
        while let Some(data) = self.pending.remove(&self.next_offset) {
            self.hasher.update(&data);
            self.next_offset = self.next_offset.saturating_add(data.len() as u64);
        }
    }

    fn try_finalize(&mut self) {
        if self.finalized.is_some() {
            return;
        }
        if self
            .size
            .map(|size| self.next_offset >= size)
            .unwrap_or(false)
        {
            self.finalized = Some(self.hasher.clone().finalize().to_hex().to_string());
        }
    }

    fn is_complete(&self) -> bool {
        self.finalized.is_some()
    }
}

impl Drop for SftpHandler {
    fn drop(&mut self) {
        let transfers = std::mem::take(&mut self.transfers);
        let state = self.state.clone();
        for transfer in transfers.into_values() {
            let state = state.clone();
            tokio::spawn(async move {
                finish_outgoing_transfer(state, transfer.job_id).await;
            });
        }
    }
}

async fn start_outgoing_transfer(
    state: &SharedState,
    peer_fp: &str,
    transfer_key: &str,
) -> Option<OutgoingTransfer> {
    let (job_id, cancel_rx, senders, peer_label) = {
        let mut guard = state.lock().await;
        if guard
            .cancelled_remote_transfers
            .contains(&(peer_fp.to_string(), transfer_key.to_string()))
        {
            return None;
        }
        let peer_name = guard
            .share_config
            .paired_peers
            .get(peer_fp)
            .map(|peer| peer.name.clone())
            .unwrap_or_else(|| "remote fbro".to_string());
        let job_id = guard.job_store.next_job_id();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (pause_tx, _pause_rx) = watch::channel(false);
        let peer_label = format!("Outgoing {job_id} -> {peer_name}");
        guard.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                notification: Default::default(),
                kind: JobKind::OutgoingTransfer(OutgoingTransferJobMeta {
                    peer_fp: peer_fp.to_string(),
                    transfer_key: transfer_key.to_string(),
                    completion_generation: 0,
                }),
                source_panel: String::new(),
                dest_panel: None,
                source_path: peer_label.clone(),
                dest_path: None,
                state: JobState::Running,
                progress: FileOpProgress {
                    status_text: Some("Sending to remote fbro".to_string()),
                    ..Default::default()
                },
                decision: None,
                error: None,
                background: true,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
                skip_current_file: Arc::new(AtomicBool::new(false)),
                keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        (job_id, cancel_rx, guard.gui_senders.clone(), peer_label)
    };

    crate::notifications::jobs::publish_start(state, &job_id).await;

    let msg = ServerMessage::JobStarted {
        timestamp: now(),
        job: JobStartSchema {
            job_id: job_id.clone(),
            kind: "outgoing-transfer".to_string(),
            op_type: "outgoing".to_string(),
            files: Vec::new(),
            source_panel: String::new(),
            source_path: peer_label,
            dest_panel: None,
            dest_path: None,
            initial_state: "Running".to_string(),
            total_files: 0,
            total_bytes_total: 0,
            background: true,
            supports_pause: false,
            result_path: None,
            result_panel_id: None,
            request_id: None,
            search_source_id: None,
            search_params: None,
        },
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }

    Some(OutgoingTransfer {
        job_id,
        cancel_rx,
        progress: FileOpProgress {
            status_text: Some("Sending to remote fbro".to_string()),
            ..Default::default()
        },
        files: Vec::new(),
        file_indexes: HashMap::new(),
        completed_files: HashSet::new(),
        file_ranges: HashMap::new(),
        file_digests: HashMap::new(),
        last_emit: Instant::now() - OUTGOING_PROGRESS_INTERVAL,
        completion_generation: 0,
    })
}

async fn emit_outgoing_progress(state: &SharedState, transfer: &mut OutgoingTransfer, force: bool) {
    if *transfer.cancel_rx.borrow() {
        return;
    }
    if !force && transfer.last_emit.elapsed() < OUTGOING_PROGRESS_INTERVAL {
        return;
    }
    transfer.last_emit = Instant::now();
    let progress = progress_schema(&transfer.progress);
    let files = transfer.files.clone();
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: transfer.job_id.clone(),
        event: "Progress".to_string(),
        files: Some(files),
        progress: Some(progress.clone()),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    };

    let senders = {
        let mut guard = state.lock().await;
        if let Some(job) = guard.job_store.jobs.get_mut(&transfer.job_id) {
            if job.state == JobState::Cancelled || *job.cancel_tx.borrow() {
                return;
            }
            // Idle completion is provisional: a later file can resume the same
            // transfer. Keep its clock, but allow the next settled end summary.
            if job.state == JobState::Completed {
                job.notification.terminal_published = false;
            }
            job.state = JobState::Running;
            job.progress = transfer.progress.clone();
            if let JobKind::OutgoingTransfer(meta) = &mut job.kind {
                meta.completion_generation = transfer.completion_generation;
            }
        }
        guard.gui_senders.clone()
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

async fn finish_outgoing_transfer(state: SharedState, job_id: String) {
    let (event, error, senders) = {
        let mut guard = state.lock().await;
        let Some(job) = guard.job_store.jobs.get_mut(&job_id) else {
            return;
        };
        let event = match job.state {
            JobState::Cancelled => return,
            JobState::Failed => "Failed",
            JobState::Completed => return,
            _ => {
                job.state = JobState::Completed;
                job.handle = None;
                "Completed"
            }
        };
        let error = job.error.clone();
        (event.to_string(), error, guard.gui_senders.clone())
    };
    crate::notifications::jobs::publish_terminal(&state, &job_id, &event, error.as_deref()).await;
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id,
        event,
        files: None,
        progress: None,
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

async fn start_remote_manifest_job(
    state: &SharedState,
    peer_fp: &str,
    root_path: &str,
    display_path: &str,
) -> (String, watch::Receiver<bool>) {
    let (job_id, cancel_rx, senders, peer_label) = {
        let mut guard = state.lock().await;
        let peer_name = guard
            .share_config
            .paired_peers
            .get(peer_fp)
            .map(|peer| peer.name.clone())
            .unwrap_or_else(|| "remote fbro".to_string());
        let job_id = guard.job_store.next_job_id();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (pause_tx, _pause_rx) = watch::channel(false);
        let peer_label = format!("Remote Sync hashing for {peer_name}: {display_path}");
        guard.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                notification: Default::default(),
                kind: JobKind::RemoteManifest(RemoteManifestJobMeta {
                    root: root_path.to_string(),
                    peer_fp: peer_fp.to_string(),
                }),
                source_panel: String::new(),
                dest_panel: None,
                source_path: peer_label.clone(),
                dest_path: None,
                state: JobState::Running,
                progress: FileOpProgress {
                    status_text: Some("Hashing for remote Sync Bro".to_string()),
                    ..Default::default()
                },
                decision: None,
                error: None,
                background: true,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
                skip_current_file: Arc::new(AtomicBool::new(false)),
                keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        (job_id, cancel_rx, guard.gui_senders.clone(), peer_label)
    };

    let msg = ServerMessage::JobStarted {
        timestamp: now(),
        job: JobStartSchema {
            job_id: job_id.clone(),
            kind: "remote-manifest".to_string(),
            op_type: "remote-manifest".to_string(),
            files: Vec::new(),
            source_panel: String::new(),
            source_path: peer_label,
            dest_panel: None,
            dest_path: None,
            initial_state: "Running".to_string(),
            total_files: 0,
            total_bytes_total: 0,
            background: true,
            supports_pause: false,
            result_path: None,
            result_panel_id: None,
            request_id: None,
            search_source_id: None,
            search_params: None,
        },
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
    (job_id, cancel_rx)
}

fn emit_remote_manifest_progress_sync(state: &SharedState, job_id: &str, progress: FileOpProgress) {
    let (senders, schema) = {
        let mut guard = state.blocking_lock();
        if let Some(job) = guard.job_store.jobs.get_mut(job_id) {
            if job.state == JobState::Cancelled || *job.cancel_tx.borrow() {
                return;
            }
            job.state = JobState::Running;
            job.progress = progress.clone();
        }
        (guard.gui_senders.clone(), progress_schema(&progress))
    };
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        files: None,
        progress: Some(schema),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

fn emit_remote_manifest_terminal_sync(
    state: &SharedState,
    job_id: &str,
    event: &str,
    error: Option<String>,
) {
    let (senders, notifications, feedback) = {
        let mut guard = state.blocking_lock();
        let feedback = if let Some(job) = guard.job_store.jobs.get_mut(job_id) {
            match event {
                "Completed" => {
                    job.state = JobState::Completed;
                    job.progress.status_text = Some("Remote Sync hashing completed".to_string());
                    job.progress.current_file = None;
                    job.progress.current_file_bytes_total = 0;
                    job.progress.current_file_bytes_copied = 0;
                    job.progress.current_file_percent = Some(100);
                    job.progress.overall_percent = Some(100);
                }
                "Cancelled" => {
                    job.state = JobState::Cancelled;
                    job.progress.status_text = Some("Remote Sync hashing cancelled".to_string());
                    job.progress.current_file = None;
                }
                "Failed" => {
                    job.state = JobState::Failed;
                    job.progress.status_text = Some("Remote Sync hashing failed".to_string());
                    job.error = error.clone();
                    job.progress.current_file = None;
                }
                _ => {}
            }
            job.paused = false;
            job.decision = None;
            job.handle = None;
            crate::notifications::jobs::terminal_feedback(job, event, error.as_deref())
        } else {
            None
        };
        (guard.gui_senders.clone(), guard.notifications.clone(), feedback)
    };
    if let Some(feedback) = feedback {
        feedback.publish(&notifications);
    }
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        files: None,
        progress: None,
        search_progress: None,
        decision: None,
        error,
        result_path: None,
        result_panel_id: None,
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

async fn emit_remote_manifest_terminal(
    state: &SharedState,
    job_id: &str,
    event: &str,
    error: Option<String>,
) {
    let (senders, notifications, feedback) = {
        let mut guard = state.lock().await;
        let feedback = if let Some(job) = guard.job_store.jobs.get_mut(job_id) {
            match event {
                "Completed" => {
                    job.state = JobState::Completed;
                    job.progress.status_text = Some("Remote Sync hashing completed".to_string());
                    job.progress.current_file = None;
                    job.progress.current_file_bytes_total = 0;
                    job.progress.current_file_bytes_copied = 0;
                    job.progress.current_file_percent = Some(100);
                    job.progress.overall_percent = Some(100);
                }
                "Cancelled" => {
                    job.state = JobState::Cancelled;
                    job.progress.status_text = Some("Remote Sync hashing cancelled".to_string());
                    job.progress.current_file = None;
                }
                "Failed" => {
                    job.state = JobState::Failed;
                    job.progress.status_text = Some("Remote Sync hashing failed".to_string());
                    job.error = error.clone();
                    job.progress.current_file = None;
                }
                _ => {}
            }
            job.paused = false;
            job.decision = None;
            job.handle = None;
            crate::notifications::jobs::terminal_feedback(job, event, error.as_deref())
        } else {
            None
        };
        (guard.gui_senders.clone(), guard.notifications.clone(), feedback)
    };
    if let Some(feedback) = feedback {
        feedback.publish(&notifications);
    }
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        files: None,
        progress: None,
        search_progress: None,
        decision: None,
        error,
        result_path: None,
        result_panel_id: None,
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

fn schedule_outgoing_completion(state: SharedState, job_id: String, generation: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(OUTGOING_COMPLETE_IDLE).await;
        let should_finish = {
            let guard = state.lock().await;
            guard
                .job_store
                .jobs
                .get(&job_id)
                .and_then(|job| match &job.kind {
                    JobKind::OutgoingTransfer(meta) => Some(
                        job.state != JobState::Cancelled
                            && !*job.cancel_tx.borrow()
                            && meta.completion_generation == generation,
                    ),
                    _ => None,
                })
                .unwrap_or(false)
        };
        if should_finish {
            finish_outgoing_transfer(state, job_id).await;
        }
    });
}

fn covered_len(ranges: &[(u64, u64)]) -> u64 {
    ranges
        .iter()
        .map(|(start, end)| end.saturating_sub(*start))
        .sum()
}

fn add_covered_range(ranges: &mut Vec<(u64, u64)>, start: u64, end: u64) -> u64 {
    if end <= start {
        return 0;
    }
    let before = covered_len(ranges);
    ranges.push((start, end));
    ranges.sort_by_key(|(start, _)| *start);
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if let Some((_, last_end)) = merged.last_mut() {
            if start <= *last_end {
                *last_end = (*last_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    *ranges = merged;
    covered_len(ranges).saturating_sub(before)
}

/// Returns `(Some(source_id), path_in_source)` for a path inside a shared
/// source, or `(None, "")` for the virtual root `/`.
/// `path_in_source` is always normalized: leading slash stripped, no
/// trailing slash unless the entire path is empty.
fn parse_path(input: &str) -> (Option<String>, String) {
    let trimmed = input.trim_start_matches('/');
    if trimmed.is_empty() {
        return (None, String::new());
    }
    match trimmed.split_once('/') {
        Some((id, rest)) => (Some(id.to_string()), rest.trim_end_matches('/').to_string()),
        None => (
            Some(trimmed.trim_end_matches('/').to_string()),
            String::new(),
        ),
    }
}

fn outgoing_transfer_key(display_path: &str) -> String {
    let mut parts = display_path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty());
    let Some(source_id) = parts.next() else {
        return "/".to_string();
    };
    let mut key = format!("/{source_id}");
    for part in parts.take(2) {
        key.push('/');
        key.push_str(part);
    }
    key
}

fn transfer_context_from_marker_path(path: &str) -> Option<String> {
    let token = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-transfer-context/")?;
    let token = token.trim_matches('/');
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(format!("client:{token}"))
}

fn transfer_status_from_path(path: &str) -> Option<String> {
    let token = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-transfer-status/")?;
    transfer_context_from_marker_path(&format!("/.fbro-transfer-context/{token}"))
}

fn integrity_result_from_path(path: &str) -> Option<(String, String)> {
    let suffix = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-integrity/result/")?;
    let (job_id, encoded_path) = suffix.split_once('/')?;
    if job_id.is_empty()
        || encoded_path.is_empty()
        || job_id.len() > 128
        || job_id.contains("..")
        || !job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some((job_id.to_string(), encoded_path.to_string()))
}

fn integrity_file_from_path(path: &str) -> Option<String> {
    let encoded_path = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-integrity/file/")?;
    if encoded_path.is_empty() {
        return None;
    }
    Some(encoded_path.to_string())
}

fn manifest_start_from_path(path: &str) -> Option<String> {
    let encoded_path = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-manifest/start/")?;
    if encoded_path.is_empty() {
        return None;
    }
    Some(encoded_path.to_string())
}

fn manifest_status_from_path(path: &str) -> Option<String> {
    let job_id = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-manifest/status/")?;
    manifest_job_id_from_suffix(job_id)
}

fn manifest_cancel_from_path(path: &str) -> Option<String> {
    let job_id = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-manifest/cancel/")?;
    manifest_job_id_from_suffix(job_id)
}

fn manifest_job_id_from_suffix(job_id: &str) -> Option<String> {
    if job_id.is_empty()
        || job_id.len() > 128
        || job_id.contains("..")
        || !job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(job_id.to_string())
}

fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether to emit per-file `[remote-manifest]` traces to stderr. Cached: these
/// fire for every file (and used to fire per chunk), so gate them behind the
/// existing spy flag to keep large-tree hashing off the stderr-write hot path.
fn manifest_debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("FBRO_SPY").is_ok())
}

fn build_manifest_on_remote(
    state: &SharedState,
    control_job_id: &str,
    driver: &dyn VfsDriver,
    root: &str,
    job: &Arc<Mutex<ManifestJobState>>,
) -> Result<(), String> {
    walk_manifest_on_remote(state, control_job_id, driver, root, "", job)?;
    let mut guard = job.blocking_lock();
    guard.entries.sort_by(|a, b| a.path.cmp(&b.path));
    guard.errors.sort_by(|a, b| a.path.cmp(&b.path));
    guard.updated_at = unix_millis_now();
    Ok(())
}

fn walk_manifest_on_remote(
    state: &SharedState,
    control_job_id: &str,
    driver: &dyn VfsDriver,
    abs_dir: &str,
    rel_dir: &str,
    job: &Arc<Mutex<ManifestJobState>>,
) -> Result<(), String> {
    if manifest_cancelled(job) {
        return Err("Cancelled".to_string());
    }
    let children = match driver.read_dir_entries(abs_dir) {
        Ok(children) => children,
        Err(error) if rel_dir.is_empty() => {
            return Err(format!("Cannot read directory {abs_dir}: {error}"));
        }
        Err(error) => {
            push_manifest_error(
                state,
                job,
                ManifestErrorResponse {
                    path: rel_dir.to_string(),
                    message: format!("Cannot read directory {abs_dir}: {error}"),
                },
            );
            return Ok(());
        }
    };

    for child in children {
        if rel_dir.is_empty() && child.name == SYNC_REPORT_NAME {
            continue;
        }
        let child_abs = driver.resolve_child(abs_dir, &child.name);
        let child_rel = if rel_dir.is_empty() {
            child.name.clone()
        } else {
            format!("{rel_dir}/{}", child.name)
        };
        if child.is_dir {
            walk_manifest_on_remote(state, control_job_id, driver, &child_abs, &child_rel, job)?;
        } else {
            if manifest_debug() {
                eprintln!(
                    "[remote-manifest] file start control_job={control_job_id} path={child_rel} size={}",
                    child.size
                );
            }
            update_manifest_progress(
                state,
                job,
                Some(child_rel.clone()),
                None,
                child.size,
                0,
                true,
            );
            match hash_file_on_remote(
                state,
                control_job_id,
                driver,
                &child_abs,
                &child_rel,
                child.size,
                job,
            ) {
                Ok((size, hash)) => push_manifest_entry(
                    state,
                    job,
                    ManifestEntryResponse {
                        path: child_rel,
                        size,
                        hash,
                    },
                ),
                Err(message) if message == "Cancelled" => return Err(message),
                Err(message) => push_manifest_error(
                    state,
                    job,
                    ManifestErrorResponse {
                        path: child_rel,
                        message,
                    },
                ),
            }
        }
    }
    Ok(())
}

fn hash_file_on_remote(
    state: &SharedState,
    control_job_id: &str,
    driver: &dyn VfsDriver,
    path: &str,
    rel_path: &str,
    size_hint: u64,
    job: &Arc<Mutex<ManifestJobState>>,
) -> Result<(u64, String), String> {
    let size = driver.file_size(path).unwrap_or(size_hint);

    // Fast path: a real local file on the remote host is hashed with a memory
    // map and multi-core BLAKE3 — the main speedup for large transferred files.
    if let Some(local) = driver.local_path(path) {
        let progress = |hashed: u64| {
            update_manifest_progress(state, job, Some(rel_path.to_string()), None, size, hashed, false);
        };
        match hash_local_file_blake3(&local, &|| manifest_cancelled(job), progress) {
            Ok(hash) => {
                if manifest_debug() {
                    eprintln!(
                        "[remote-manifest] file complete control_job={control_job_id} path={rel_path}"
                    );
                }
                return Ok((size, hash));
            }
            Err(error) if error.to_string().contains("Cancelled") => {
                return Err("Cancelled".to_string());
            }
            Err(error) => return Err(format!("Cannot digest {path}: {error}")),
        }
    }

    match hash_file_on_remote_streaming(state, control_job_id, driver, path, rel_path, size, job) {
        Err(message) if message.starts_with("streaming unsupported:") => {}
        result => return result,
    }

    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    while offset < size {
        if manifest_cancelled(job) {
            if manifest_debug() {
                eprintln!(
                    "[remote-manifest] file cancelled control_job={control_job_id} path={rel_path}"
                );
            }
            return Err("Cancelled".to_string());
        }
        let remaining = size
            .saturating_sub(offset)
            .min(MANIFEST_HASH_CHUNK_SIZE as u64) as usize;
        let chunk = driver
            .read_chunk(path, offset, remaining)
            .map_err(|error| format!("Cannot read {path}: {error}"))?;
        if chunk.is_empty() {
            break;
        }
        offset = offset.saturating_add(chunk.len() as u64);
        hasher.update(&chunk);
        update_manifest_progress(
            state,
            job,
            Some(rel_path.to_string()),
            None,
            size,
            offset,
            false,
        );
    }
    if offset != size {
        if manifest_debug() {
            eprintln!(
                "[remote-manifest] file short-read control_job={control_job_id} path={rel_path} offset={offset} size={size}"
            );
        }
        return Err(format!("digest read ended at {offset} of {size} bytes"));
    }
    if manifest_debug() {
        eprintln!("[remote-manifest] file complete control_job={control_job_id} path={rel_path}");
    }
    Ok((size, hasher.finalize().to_hex().to_string()))
}

fn hash_file_on_remote_streaming(
    state: &SharedState,
    control_job_id: &str,
    driver: &dyn VfsDriver,
    path: &str,
    rel_path: &str,
    size: u64,
    job: &Arc<Mutex<ManifestJobState>>,
) -> Result<(u64, String), String> {
    let mut reader = match driver.open_read(path) {
        Ok(reader) => reader,
        Err(VfsError::NotSupported(message)) => {
            return Err(format!("streaming unsupported: {message}"));
        }
        Err(error) => return Err(format!("Cannot open {path}: {error}")),
    };

    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut buf = vec![0u8; MANIFEST_STREAM_BUF_SIZE];
    loop {
        if manifest_cancelled(job) {
            if manifest_debug() {
                eprintln!(
                    "[remote-manifest] file cancelled control_job={control_job_id} path={rel_path}"
                );
            }
            return Err("Cancelled".to_string());
        }
        let n = reader
            .read(&mut buf)
            .map_err(|error| format!("Cannot read {path}: {error}"))?;
        if n == 0 {
            break;
        }
        offset = offset.saturating_add(n as u64);
        hasher.update(&buf[..n]);
        update_manifest_progress(
            state,
            job,
            Some(rel_path.to_string()),
            None,
            size,
            offset,
            false,
        );
    }
    if offset != size {
        if manifest_debug() {
            eprintln!(
                "[remote-manifest] file short-read control_job={control_job_id} path={rel_path} offset={offset} size={size}"
            );
        }
        return Err(format!("digest read ended at {offset} of {size} bytes"));
    }
    if manifest_debug() {
        eprintln!("[remote-manifest] file complete control_job={control_job_id} path={rel_path}");
    }
    Ok((size, hasher.finalize().to_hex().to_string()))
}

fn manifest_cancelled(job: &Arc<Mutex<ManifestJobState>>) -> bool {
    let guard = job.blocking_lock();
    guard.cancelled
        || guard
            .visible_cancel_rx
            .as_ref()
            .map(|rx| *rx.borrow())
            .unwrap_or(false)
}

fn update_manifest_progress(
    state: &SharedState,
    job: &Arc<Mutex<ManifestJobState>>,
    current_path: Option<String>,
    processed: Option<usize>,
    current_file_bytes_total: u64,
    current_file_bytes_hashed: u64,
    force_emit: bool,
) {
    // The lock fields (updated_at, byte counters) are refreshed on every call so
    // the status endpoint and stale-timeout see live data, but the status-string
    // allocation and progress struct are only built when we actually emit (this
    // runs per chunk on large files).
    let (visible_job_id, progress) = {
        let mut guard = job.blocking_lock();
        if let Some(processed) = processed {
            guard.processed = processed;
        }
        guard.current_path = current_path;
        guard.current_file_bytes_total = current_file_bytes_total;
        guard.current_file_bytes_hashed = current_file_bytes_hashed;
        guard.updated_at = unix_millis_now();
        let now = Instant::now();
        let should_emit = force_emit
            || guard
                .last_emit
                .map(|last| now.duration_since(last) >= OUTGOING_PROGRESS_INTERVAL)
                .unwrap_or(true);
        if !should_emit {
            return;
        }
        guard.last_emit = Some(now);
        let status_text = guard
            .current_path
            .as_ref()
            .map(|path| format!("Hashing for remote Sync Bro: {path}"))
            .unwrap_or_else(|| "Hashing for remote Sync Bro".to_string());
        (
            guard.visible_job_id.clone(),
            FileOpProgress {
                total_files: 0,
                completed_files: guard.processed,
                current_file: guard.current_path.clone(),
                status_text: Some(status_text),
                current_file_bytes_total: guard.current_file_bytes_total,
                current_file_bytes_copied: guard.current_file_bytes_hashed,
                current_file_percent: percent(
                    guard.current_file_bytes_hashed,
                    guard.current_file_bytes_total,
                ),
                ..Default::default()
            },
        )
    };
    emit_remote_manifest_progress_sync(state, &visible_job_id, progress);
}

fn push_manifest_entry(
    state: &SharedState,
    job: &Arc<Mutex<ManifestJobState>>,
    entry: ManifestEntryResponse,
) {
    let path = entry.path.clone();
    let size = entry.size;
    {
        let mut guard = job.blocking_lock();
        guard.entries.push(entry);
        guard.processed = guard.entries.len() + guard.errors.len();
    }
    update_manifest_progress(state, job, Some(path), None, size, size, true);
}

fn push_manifest_error(
    state: &SharedState,
    job: &Arc<Mutex<ManifestJobState>>,
    error: ManifestErrorResponse,
) {
    eprintln!(
        "[remote-manifest] file error path={} message={}",
        error.path, error.message
    );
    let path = error.path.clone();
    {
        let mut guard = job.blocking_lock();
        guard.errors.push(error);
        guard.processed = guard.entries.len() + guard.errors.len();
    }
    update_manifest_progress(state, job, Some(path), None, 0, 0, true);
}

fn is_sources_metadata_path(path: &str) -> bool {
    path.trim_start_matches('/').trim_end_matches('/') == ".fbro-sources.json"
}

fn thumbnail_request_from_path(path: &str) -> Option<(u32, String)> {
    let suffix = path
        .trim_start_matches('/')
        .strip_prefix(".fbro-thumbnail/")?;
    let (size_str, encoded_path) = suffix.split_once('/')?;
    if encoded_path.is_empty() || encoded_path.contains('/') {
        return None;
    }
    let size = size_str.parse::<u32>().ok()?.clamp(32, 512);
    Some((size, encoded_path.to_string()))
}

/// Reject path-traversal — any `..` segment, after normalization, is a
/// traversal attempt and must be refused. (defence in depth on top of
/// the driver's own canonicalization.)
fn has_dotdot(path: &str) -> bool {
    path.split('/').any(|seg| seg == "..")
}

fn share_root_available(state: &crate::state::AppState, root: &crate::state::SharedRoot) -> bool {
    state.drivers.contains_key(&root.source_id)
        && (matches!(root.origin, crate::state::SharedRootOrigin::Folder)
            || state.sources.contains_key(&root.source_id))
}

async fn resolve_source(
    state: &SharedState,
    root_id: &str,
) -> Option<(Arc<dyn VfsDriver>, String, String, String, Option<String>)> {
    let guard = state.lock().await;
    let root = guard
        .share_config
        .shared_roots
        .iter()
        .find(|root| root.id == root_id)?;
    let driver = guard.drivers.get(&root.source_id)?.clone();
    Some((
        driver,
        root.path.clone(),
        root.source_id.clone(),
        root.name.clone(),
        root.icon_name.clone(),
    ))
}

fn vfs_err_to_status(e: VfsError) -> StatusCode {
    match e {
        VfsError::Cancelled => StatusCode::PermissionDenied,
        VfsError::NotFound(_) => StatusCode::NoSuchFile,
        VfsError::PermissionDenied(_) => StatusCode::PermissionDenied,
        VfsError::AlreadyExists(_) => StatusCode::Failure,
        VfsError::NotSupported(_) => StatusCode::OpUnsupported,
        VfsError::IOError(_) => StatusCode::Failure,
    }
}

// russh-sftp's `FileAttributes::default()` sets `permissions: Some(0o777 |
// FileMode::DIR)` — i.e. the default is a directory. `set_regular(true)`
// then ORs the REG bit on top of the still-set DIR bit, so the client's
// `is_dir()` keeps returning true for regular files. We have to either
// reset the file-type bits ourselves or build attrs from scratch.
const TYPE_BITS_MASK: u32 = 0xF000;

fn entry_to_file(entry: &DirEntry) -> SftpFile {
    let attrs = if entry.is_dir {
        attrs_for_dir_with(entry.size, entry.mtime)
    } else {
        attrs_for_file_with(entry.size, entry.mtime)
    };
    SftpFile::new(entry.name.clone(), attrs)
}

fn attrs_for_dir_with(size: Option<u64>, mtime: Option<u32>) -> FileAttributes {
    let mut a = FileAttributes::default();
    // Drop the default's stale DIR bit before setting our own — even though
    // they're the same here, it keeps the helper interchangeable with the
    // file case and removes the implicit reliance on default's quirk.
    a.permissions = Some(strip_type(a.permissions.unwrap_or(0)) | 0x4000);
    a.size = size;
    a.mtime = mtime;
    a
}

fn attrs_for_file_with(size: Option<u64>, mtime: Option<u32>) -> FileAttributes {
    let mut a = FileAttributes::default();
    // Strip the DIR bit baked into default(), then set REG.
    a.permissions = Some(strip_type(a.permissions.unwrap_or(0)) | 0x8000);
    a.size = size;
    a.mtime = mtime;
    a
}

fn strip_type(perms: u32) -> u32 {
    perms & !TYPE_BITS_MASK
}

fn attrs_for_dir() -> FileAttributes {
    attrs_for_dir_with(None, None)
}

fn attrs_for_file(size: Option<u64>) -> FileAttributes {
    attrs_for_file_with(size, None)
}

fn mtime_unix(s: Option<&str>) -> Option<u32> {
    // VfsEntry mtime is stored as a string of unix seconds (see
    // vfs::format_mtime). Parse defensively — if it doesn't look like a
    // number, just drop it; SFTP attributes are optional.
    s.and_then(|x| x.parse::<u64>().ok())
        .and_then(|secs| u32::try_from(secs).ok())
}

impl russh_sftp::server::Handler for SftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        if self.version.is_some() {
            return Err(StatusCode::PermissionDenied);
        }
        self.version = Some(version);
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        // We don't do server-side path normalization — return the path as-is.
        // For "" or relative paths the client expects, coerce to "/".
        let normalized = if path.is_empty() || path == "." {
            "/".to_string()
        } else {
            path
        };
        Ok(Name {
            id,
            files: vec![SftpFile::dummy(normalized)],
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if let Some(HandleState::File {
            display_path,
            transfer_key,
            size,
            completed,
            ..
        }) = self.handles.remove(&handle)
        {
            let completed = completed || !self.transfer_cancelled_for(&transfer_key).await;
            self.note_file_closed(&transfer_key, &display_path, size, completed)
                .await;
        } else {
            self.handles.remove(&handle);
        }
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        })
    }

    async fn opendir(&mut self, _id: u32, path: String) -> Result<Handle, Self::Error> {
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let (source_id, rest) = parse_path(&path);

        let entries: Vec<DirEntry> = if let Some(sid) = source_id.as_deref() {
            let Some((driver, root, _, _name, _icon)) = resolve_source(&self.state, sid).await
            else {
                return Err(StatusCode::NoSuchFile);
            };
            let driver_path = compose_driver_path(&root, &rest);
            let sort = crate::ws::protocol::SortSpec {
                by: "name".to_string(),
                order: "asc".to_string(),
            };
            let listing = driver
                .list(&driver_path, &sort, None, true)
                .map_err(vfs_err_to_status)?;
            listing
                .into_iter()
                .map(|e| DirEntry {
                    is_dir: matches!(e.kind, VfsKind::Directory),
                    size: e.size,
                    mtime: mtime_unix(e.mtime.as_deref()),
                    name: e.name,
                })
                .collect()
        } else {
            // Virtual root: synthesize a directory per shared source id.
            let guard = self.state.lock().await;
            let mut shared: Vec<crate::state::SharedRoot> =
                guard.share_config.shared_roots.iter().cloned().collect();
            shared.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            shared
                .into_iter()
                .filter(|root| share_root_available(&guard, root))
                .map(|root| DirEntry {
                    name: root.id,
                    is_dir: true,
                    size: None,
                    mtime: None,
                })
                .collect()
        };

        let handle = self.fresh_handle();
        self.handles.insert(
            handle.clone(),
            HandleState::Dir {
                source_id,
                path_in_source: rest,
                entries,
                cursor: 0,
            },
        );
        Ok(Handle { id: _id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let state = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        let HandleState::Dir {
            entries, cursor, ..
        } = state
        else {
            return Err(StatusCode::Failure);
        };
        if *cursor >= entries.len() {
            return Err(StatusCode::Eof);
        }
        // Return everything in a single batch — the example does the same.
        // Clients call readdir repeatedly until Eof; we send all entries
        // then EOF on the next call.
        let files: Vec<SftpFile> = entries[*cursor..].iter().map(entry_to_file).collect();
        *cursor = entries.len();
        Ok(Name { id, files })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if has_dotdot(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let (source_id, rest) = parse_path(&path);
        let attrs = match source_id {
            None => attrs_for_dir(), // virtual root
            Some(sid) => {
                let Some((driver, root, _, _, _)) = resolve_source(&self.state, &sid).await else {
                    return Err(StatusCode::NoSuchFile);
                };
                let driver_path = compose_driver_path(&root, &rest);
                if !driver.exists(&driver_path) {
                    return Err(StatusCode::NoSuchFile);
                }
                // If listing the parent + finding ourselves is cheaper than
                // dedicated stat, we could; for now: use file_size + exists.
                let size = driver.file_size(&driver_path);
                // Heuristic: file_size==None on directories for most drivers.
                if size.is_some() {
                    attrs_for_file(size)
                } else {
                    attrs_for_dir()
                }
            }
        };
        Ok(Attrs { id, attrs })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        // We don't expose symlinks — lstat == stat.
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let state = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        let attrs = match state {
            HandleState::File { size, .. } => attrs_for_file(*size),
            HandleState::Dir { .. } => attrs_for_dir(),
            HandleState::Control => attrs_for_file(Some(0)),
            HandleState::ControlData { data } => attrs_for_file(Some(data.len() as u64)),
        };
        Ok(Attrs { id, attrs })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        // Read-only — refuse any flag involving writes.
        if pflags.intersects(
            OpenFlags::WRITE | OpenFlags::APPEND | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        ) {
            return Err(StatusCode::PermissionDenied);
        }
        if has_dotdot(&filename) {
            return Err(StatusCode::PermissionDenied);
        }
        if let Some(transfer_key) = transfer_status_from_path(&filename) {
            let cancelled = self.transfer_cancelled_for(&transfer_key).await;
            let data = serde_json::to_vec(&serde_json::json!({ "cancelled": cancelled }))
                .map_err(|_| StatusCode::Failure)?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some(transfer_key) = transfer_context_from_marker_path(&filename) {
            self.session_transfer_key = Some(transfer_key);
            let handle = self.fresh_handle();
            self.handles.insert(handle.clone(), HandleState::Control);
            return Ok(Handle { id, handle });
        }
        if let Some((job_id, encoded_path)) = integrity_result_from_path(&filename) {
            let data = self.integrity_result_json(&job_id, &encoded_path)?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some(encoded_path) = integrity_file_from_path(&filename) {
            let data = self.full_file_digest_json(&encoded_path).await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some(encoded_path) = manifest_start_from_path(&filename) {
            let data = self.start_manifest_json(&encoded_path).await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some(job_id) = manifest_status_from_path(&filename) {
            let data = self.manifest_status_json(&job_id).await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some(job_id) = manifest_cancel_from_path(&filename) {
            let data = self.manifest_cancel_json(&job_id).await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if is_sources_metadata_path(&filename) {
            let data = self.sources_metadata_json().await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        if let Some((size, encoded_path)) = thumbnail_request_from_path(&filename) {
            let data = self.thumbnail_bytes(size, &encoded_path).await?;
            let handle = self.fresh_handle();
            self.handles
                .insert(handle.clone(), HandleState::ControlData { data });
            return Ok(Handle { id, handle });
        }
        let (source_id, rest) = parse_path(&filename);
        let Some(sid) = source_id else {
            return Err(StatusCode::NoSuchFile);
        };
        let Some((driver, root, actual_source_id, _, _)) = resolve_source(&self.state, &sid).await
        else {
            return Err(StatusCode::NoSuchFile);
        };
        let display_path = format!("/{sid}/{}", rest).trim_end_matches('/').to_string();
        let transfer_key = self
            .session_transfer_key
            .clone()
            .unwrap_or_else(|| outgoing_transfer_key(&display_path));
        if self.transfer_cancelled_for(&transfer_key).await {
            return Err(StatusCode::PermissionDenied);
        }
        let driver_path = compose_driver_path(&root, &rest);
        if !driver.exists(&driver_path) {
            return Err(StatusCode::NoSuchFile);
        }
        let size = driver.file_size(&driver_path);
        let handle = self.fresh_handle();
        self.handles.insert(
            handle.clone(),
            HandleState::File {
                source_id: actual_source_id,
                path_in_source: driver_path,
                display_path: display_path.clone(),
                transfer_key: transfer_key.clone(),
                size,
                completed: false,
            },
        );
        self.note_file_opened(&transfer_key, &display_path, size)
            .await;
        if self.transfer_cancelled_for(&transfer_key).await {
            self.handles.remove(&handle);
            return Err(StatusCode::PermissionDenied);
        }
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let state = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        if let HandleState::ControlData { data } = state {
            let start = offset as usize;
            if start >= data.len() {
                return Err(StatusCode::Eof);
            }
            let end = start.saturating_add(len as usize).min(data.len());
            return Ok(Data {
                id,
                data: data[start..end].to_vec(),
            });
        }
        let HandleState::File {
            source_id,
            path_in_source,
            display_path,
            transfer_key,
            size,
            ..
        } = state
        else {
            return Err(match state {
                HandleState::Control | HandleState::ControlData { .. } => StatusCode::Eof,
                _ => StatusCode::Failure,
            });
        };
        let source_id = source_id.clone();
        let path = path_in_source.clone();
        let display_path = display_path.clone();
        let transfer_key = transfer_key.clone();
        let total = *size;
        if self.transfer_cancelled_for(&transfer_key).await {
            return Err(StatusCode::PermissionDenied);
        }

        // EOF detection upfront avoids a driver round-trip when the client
        // is just probing past the end.
        if let Some(total) = total {
            if offset >= total {
                return Err(StatusCode::Eof);
            }
        }
        let driver = {
            let guard = self.state.lock().await;
            guard.drivers.get(&source_id).cloned()
        };
        let Some(driver) = driver else {
            return Err(StatusCode::NoSuchFile);
        };
        // Driver methods are sync — push to spawn_blocking so we don't park
        // the tokio worker on what may be a slow read.
        let data =
            tokio::task::spawn_blocking(move || driver.read_chunk(&path, offset, len as usize))
                .await
                .map_err(|_| StatusCode::Failure)?
                .map_err(vfs_err_to_status)?;
        if self.transfer_cancelled_for(&transfer_key).await {
            return Err(StatusCode::PermissionDenied);
        }
        if data.is_empty() {
            return Err(StatusCode::Eof);
        }
        let read_len = data.len();
        self.note_file_read(&transfer_key, &display_path, offset, &data, read_len, total)
            .await;
        if self.transfer_cancelled_for(&transfer_key).await {
            return Err(StatusCode::PermissionDenied);
        }
        Ok(Data { id, data })
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        _data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        if request != "fbro@anthropic.com/list-sources" {
            return Err(StatusCode::OpUnsupported);
        }
        let guard = self.state.lock().await;
        let stream_thumbnails = guard.settings.general.stream_thumbnails;
        let mut items: Vec<serde_json::Value> = Vec::new();
        for root in guard.share_config.shared_roots.iter() {
            if let Some(driver) = guard.drivers.get(&root.source_id) {
                if !share_root_available(&guard, root) {
                    continue;
                }
                items.push(serde_json::json!({
                    "id": root.id,
                    "name": root.name,
                    "icon": root.icon_name,
                    "read_only": true,
                    "supports_thumbnails": stream_thumbnails && driver.supports_thumbnails(),
                    "supports_manifest": true,
                }));
            }
        }
        let body = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        // Return as a STATUS packet with the body in the message field — the
        // simplest "extended response" shape that survives the SFTP framing
        // without needing a custom packet type. The client parses
        // `status.error_message` as JSON.
        Ok(Packet::Status(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: body,
            language_tag: "en-US".to_string(),
        }))
    }
}

fn compose_driver_path(root: &str, rest: &str) -> String {
    match (root.is_empty(), rest.is_empty()) {
        (true, true) => String::new(),
        (true, false) => rest.to_string(),
        (false, true) => root.to_string(),
        (false, false) => {
            if root.ends_with('/') {
                format!("{root}{rest}")
            } else {
                format!("{root}/{rest}")
            }
        }
    }
}

// Keep one reference to UNIX_EPOCH so the import isn't flagged when feature
// flags shrink the file. The mtime parsing path uses it indirectly.
#[allow(dead_code)]
const _EPOCH: SystemTime = UNIX_EPOCH;

#[cfg(test)]
mod tests;
