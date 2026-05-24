//! Client-side SFTP driver for paired fbro peers.
//!
//! Each instance represents *one source on one peer* — i.e. a `Source` with
//! `kind == "remote"`. The driver owns:
//!  - a dedicated multi-thread tokio runtime that drives all russh I/O,
//!  - a long-lived **session supervisor** task that establishes,
//!    heartbeats, and reconnects the russh + SFTP session,
//!  - a `StatusBroadcaster` used to push `RemoteSourceStatus` events to
//!    the FE as the supervisor transitions between states.
//!
//! VfsDriver methods are synchronous. Each one acquires the session
//! state, clones the live `Arc<SftpSession>` if Ready (otherwise waits
//! briefly for the supervisor to recover or returns an `IOError`), and
//! `block_on`s a small async task on the dedicated runtime. That runtime
//! is separate from the main app runtime, so calling `block_on` from any
//! caller thread (including tokio blocking-pool threads) won't deadlock.
//!
//! v1 is read-only: writes / removes / renames return `NotSupported`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelId;
use russh_sftp::client::{Config as SftpClientConfig, SftpSession};
use serde::Deserialize;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, Mutex, Notify};

use crate::remote::keys::{fingerprint_of, Identity};
use crate::vfs::{
    color_class_for_entry, format_mtime, icon_name_for_entry, DirChild, VfsDriver, VfsEntry,
    VfsError, VfsKind, VfsTransferDigest,
};
use crate::ws::protocol::{FilterSpec, SortSpec};

/// Connection parameters baked in at driver construction.
pub struct SftpDriverConfig {
    pub host: String,
    pub port: u16,
    /// Username sent in SSH auth. The server ignores it — auth is by pubkey.
    pub user: String,
    /// Our local identity (signing key). Same key the embedded server
    /// advertises as its host key.
    pub identity: Arc<Identity>,
    /// Expected fingerprint of the remote host's pubkey. The client refuses
    /// the connection if the presented host key doesn't match — equivalent
    /// to a known_hosts pin tied to the paired peer's id.
    pub expected_peer_fp: String,
    /// SFTP path prefix that this driver scopes to. Always starts with `/`
    /// (e.g. `/source_id`). Empty / `/` means the virtual root.
    pub path_prefix: String,
    /// Source id used when emitting `RemoteSourceStatus` events. Empty
    /// string for tests / unattached usage.
    pub source_id: String,
    /// When false, connection setup only uses IPv4 socket addresses.
    pub allow_ipv6: bool,
}

/// Heartbeat cadence: 10s, comfortably under russh's 60s
/// `inactivity_timeout`. Cheap (~20 bytes RTT for `canonicalize(".")`).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// Heartbeat watchdog: if a ping doesn't return in this much time the
/// session is considered dead.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum backoff between reconnect attempts. Cap rather than letting
/// it grow unboundedly — once we're at the cap we keep trying every
/// `RECONNECT_MAX_BACKOFF` indefinitely so the source can recover the
/// moment the peer comes back.
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// How long a synchronous trait method will wait for the supervisor to
/// transition to Ready before giving up with `IOError("reconnecting")`.
/// Kept short so a navigation click in the UI doesn't hang.
const TRAIT_METHOD_WAIT: Duration = Duration::from_secs(3);
const SSH_CHANNEL_WINDOW_SIZE: u32 = 16 * 1024 * 1024;
const SFTP_CLIENT_MAX_PACKET_LEN: u32 = 1024 * 1024;
const SFTP_READ_CHUNK_SIZE: usize = 256 * 1024;
const SFTP_READ_AHEAD_CHUNKS: usize = 16;
const FBRO_INTEGRITY_ALGORITHM: &str = "blake3";

thread_local! {
    static CURRENT_FILE_OP_JOB: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub struct SftpFsDriver {
    /// Boxed in `Option` solely so `Drop` can move it out and call
    /// `shutdown_background()`. The default `Runtime::drop` is a *blocking*
    /// shutdown (it joins worker threads), which panics when the driver is
    /// dropped from inside another tokio runtime.
    runtime: Option<Runtime>,
    config: Arc<SftpDriverConfig>,
    state: Arc<DriverState>,
}

/// Shared state between the supervisor and trait methods.
struct DriverState {
    session: Mutex<SessionState>,
    /// Wakes trait callers when the session transitions to Ready.
    on_ready: Notify,
    /// Signaled by trait methods when an operation failed with a
    /// connection-fatal error. The supervisor drops the session and
    /// transitions to Reconnecting.
    broken: Notify,
    /// Signaled by `force_reconnect()` or `ReconnectRemoteSource`
    /// intents to short-circuit the supervisor's backoff sleep.
    force_reconnect: Notify,
    /// Signaled in `Drop` so the supervisor task can exit cleanly. (The
    /// runtime is dropped anyway, but a clean exit means no log noise
    /// from canceled in-flight requests.)
    shutdown: Notify,
    /// Dedicated SFTP sessions for active local file-operation jobs. This
    /// lets the remote fbro see separate outgoing transfer jobs even when
    /// two local copy jobs read from the same mounted remote source.
    job_connections: Mutex<HashMap<String, Arc<Connection>>>,
}

enum SessionState {
    /// First state. The supervisor moves us out of Idle on startup.
    Idle,
    /// A connect attempt is in flight.
    Connecting,
    /// We have a live session.
    Ready { conn: Arc<Connection> },
    /// We had a session (or initial connect failed) and we're waiting
    /// the configured backoff before the next attempt.
    Reconnecting { attempt: u32, next_at: Instant },
    /// Unused in v1 — kept so the state machine has somewhere terminal
    /// to land if a fatal/unrecoverable error is ever introduced (e.g.
    /// peer fingerprint mismatch). For now Reconnecting always retries.
    #[allow(dead_code)]
    Dead { reason: String },
}

struct Connection {
    /// Kept so the russh session stays alive — the SFTP subsystem is a
    /// channel inside it. Drop = clean disconnect.
    _handle: russh::client::Handle<ClientHandler>,
    sftp: Arc<SftpSession>,
}

// ─── Public broadcaster trait ─────────────────────────────────────────────────

/// Hook the caller passes in so the supervisor can push
/// `RemoteSourceStatus` events to the FE without owning the WS plumbing.
pub trait StatusBroadcaster: Send + Sync {
    fn emit(
        &self,
        source_id: &str,
        status: &str,
        message: Option<String>,
        next_retry_at: Option<u64>,
    );
}

/// No-op broadcaster for tests / detached usage.
pub struct NoopStatusBroadcaster;
impl StatusBroadcaster for NoopStatusBroadcaster {
    fn emit(&self, _: &str, _: &str, _: Option<String>, _: Option<u64>) {}
}

#[derive(Deserialize)]
struct RemoteDigestResponse {
    algorithm: String,
    size: u64,
    hash: String,
    complete: bool,
}

// ─── Driver lifecycle ─────────────────────────────────────────────────────────

impl SftpFsDriver {
    pub fn new(config: SftpDriverConfig, status: Arc<dyn StatusBroadcaster>) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("fbro-sftp")
            .build()?;
        let config = Arc::new(config);
        let state = Arc::new(DriverState {
            session: Mutex::new(SessionState::Idle),
            on_ready: Notify::new(),
            broken: Notify::new(),
            force_reconnect: Notify::new(),
            shutdown: Notify::new(),
            job_connections: Mutex::new(HashMap::new()),
        });
        // Spawn the supervisor on the dedicated runtime so it stays alive
        // for the driver's lifetime. The runtime's `shutdown_background()`
        // (called from Drop) cancels it.
        {
            let config = config.clone();
            let state = state.clone();
            let status = status.clone();
            runtime.spawn(async move { supervisor(config, state, status).await });
        }
        Ok(Self {
            runtime: Some(runtime),
            config,
            state,
        })
    }

    /// Force an immediate reconnect attempt, skipping any pending
    /// backoff. Called by `ReconnectRemoteSource` intent.
    pub fn force_reconnect(&self) {
        self.state.force_reconnect.notify_waiters();
    }

    /// Convert a VfsDriver-facing path into an SFTP absolute path with
    /// the configured prefix applied.
    fn absolute(&self, path: &str) -> String {
        let prefix = self.config.path_prefix.trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        match (prefix.is_empty(), suffix.is_empty()) {
            (true, true) => "/".to_string(),
            (true, false) => format!("/{suffix}"),
            (false, true) => prefix.to_string(),
            (false, false) => format!("{prefix}/{suffix}"),
        }
    }

    #[inline]
    fn rt(&self) -> &Runtime {
        self.runtime.as_ref().expect("runtime live until Drop")
    }

    /// Wait for the session to be Ready (or fail), then invoke `f` with
    /// the SFTP session handle. On a connection-fatal error from `f`,
    /// signal the supervisor and surface as `IOError("disconnected")`.
    async fn with_sftp<F, Fut, T>(&self, f: F) -> Result<T, VfsError>
    where
        F: FnOnce(Arc<SftpSession>) -> Fut,
        Fut: std::future::Future<Output = Result<T, russh_sftp::client::error::Error>>,
    {
        let job_id = current_file_op_job_id();
        let sftp = if let Some(job_id) = job_id.as_deref() {
            self.job_sftp(job_id).await?
        } else {
            self.shared_sftp().await?
        };

        match f(sftp).await {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_fatal(&e) {
                    if let Some(job_id) = job_id {
                        self.drop_job_connection(&job_id).await;
                    } else {
                        self.state.broken.notify_waiters();
                    }
                    Err(VfsError::IOError(format!("disconnected: {e}")))
                } else {
                    Err(map_sftp_err(e))
                }
            }
        }
    }

    async fn shared_sftp(&self) -> Result<Arc<SftpSession>, VfsError> {
        let deadline = Instant::now() + TRAIT_METHOD_WAIT;
        Ok(loop {
            let need_kick;
            let immediate;
            {
                let g = self.state.session.lock().await;
                match &*g {
                    SessionState::Ready { conn } => {
                        immediate = Some(conn.sftp.clone());
                        need_kick = false;
                    }
                    SessionState::Idle => {
                        // First request — wake the supervisor to start
                        // connecting. The supervisor moves Idle → Connecting
                        // → Ready / Reconnecting on its own.
                        immediate = None;
                        need_kick = true;
                    }
                    SessionState::Connecting | SessionState::Reconnecting { .. } => {
                        immediate = None;
                        need_kick = false;
                    }
                    SessionState::Dead { reason } => {
                        return Err(VfsError::IOError(format!("dead: {reason}")));
                    }
                }
            }
            if let Some(s) = immediate {
                break s;
            }
            if need_kick {
                self.state.force_reconnect.notify_waiters();
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(VfsError::IOError("reconnecting…".to_string()));
            }
            let wait = deadline - now;
            // notify_waiters only signals tasks already awaiting; we
            // call notified() *before* checking state to avoid a missed
            // wakeup. The supervisor calls notify_waiters() on every
            // transition to Ready.
            let _ = tokio::time::timeout(wait, self.state.on_ready.notified()).await;
        })
    }

    async fn job_sftp(&self, job_id: &str) -> Result<Arc<SftpSession>, VfsError> {
        {
            let guard = self.state.job_connections.lock().await;
            if let Some(conn) = guard.get(job_id) {
                return Ok(conn.sftp.clone());
            }
        }

        let conn = Arc::new(connect(&self.config).await?);
        let mut guard = self.state.job_connections.lock().await;
        let conn = guard
            .entry(job_id.to_string())
            .or_insert_with(|| conn)
            .clone();
        Ok(conn.sftp.clone())
    }

    async fn drop_job_connection(&self, job_id: &str) {
        let conn = {
            let mut guard = self.state.job_connections.lock().await;
            guard.remove(job_id)
        };
        if let Some(conn) = conn {
            let _ = conn.sftp.close().await;
        }
    }
}

impl Drop for SftpFsDriver {
    fn drop(&mut self) {
        self.state.shutdown.notify_waiters();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

// ─── Supervisor ───────────────────────────────────────────────────────────────

async fn supervisor(
    config: Arc<SftpDriverConfig>,
    state: Arc<DriverState>,
    status: Arc<dyn StatusBroadcaster>,
) {
    // Bootstrap: immediately try to connect (transitions through
    // Connecting on the way to Ready or Reconnecting).
    {
        let mut g = state.session.lock().await;
        *g = SessionState::Connecting;
    }
    status.emit(&config.source_id, "reconnecting", None, None);

    loop {
        // Snapshot the current state. We hold the lock only long enough
        // to inspect — none of the actions below need to keep callers
        // waiting on a guard.
        enum Action {
            ConnectNow,
            HeartbeatAndWatch(Arc<SftpSession>),
            BackoffWait { wait: Duration, attempt: u32 },
            WaitForKick,
        }
        let action = {
            let g = state.session.lock().await;
            match &*g {
                SessionState::Idle => Action::WaitForKick,
                SessionState::Connecting => Action::ConnectNow,
                SessionState::Ready { conn } => Action::HeartbeatAndWatch(conn.sftp.clone()),
                SessionState::Reconnecting { attempt, next_at } => {
                    let wait = next_at
                        .checked_duration_since(Instant::now())
                        .unwrap_or(Duration::ZERO);
                    Action::BackoffWait {
                        wait,
                        attempt: *attempt,
                    }
                }
                SessionState::Dead { .. } => Action::WaitForKick,
            }
        };

        match action {
            Action::ConnectNow => {
                eprintln!(
                    "[sftp] {}: connecting to {}:{}",
                    config.source_id, config.host, config.port
                );
                match connect(&config).await {
                    Ok(conn) => {
                        let mut g = state.session.lock().await;
                        *g = SessionState::Ready {
                            conn: Arc::new(conn),
                        };
                        drop(g);
                        state.on_ready.notify_waiters();
                        status.emit(&config.source_id, "connected", None, None);
                        eprintln!("[sftp] {}: connected", config.source_id);
                    }
                    Err(e) => {
                        let attempt = 1u32;
                        let next_at = Instant::now() + backoff_for(attempt);
                        let mut g = state.session.lock().await;
                        *g = SessionState::Reconnecting { attempt, next_at };
                        drop(g);
                        let msg = e.to_string();
                        eprintln!("[sftp] {}: initial connect failed: {msg}", config.source_id);
                        status.emit(
                            &config.source_id,
                            "reconnecting",
                            Some(msg),
                            Some(unix_millis_at(next_at)),
                        );
                    }
                }
            }
            Action::HeartbeatAndWatch(sftp) => {
                tokio::select! {
                    _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {
                        let probe = tokio::time::timeout(
                            HEARTBEAT_TIMEOUT,
                            sftp.canonicalize("."),
                        ).await;
                        let dead = !matches!(probe, Ok(Ok(_)));
                        if dead {
                            eprintln!(
                                "[sftp] {}: heartbeat failed",
                                config.source_id
                            );
                            let mut g = state.session.lock().await;
                            *g = SessionState::Reconnecting {
                                attempt: 0,
                                next_at: Instant::now(),
                            };
                            drop(g);
                            status.emit(
                                &config.source_id,
                                "disconnected",
                                Some("heartbeat failed".to_string()),
                                None,
                            );
                        }
                        // healthy: fall through to the loop top
                    }
                    _ = state.broken.notified() => {
                        eprintln!("[sftp] {}: session marked broken", config.source_id);
                        let mut g = state.session.lock().await;
                        if matches!(*g, SessionState::Ready { .. }) {
                            *g = SessionState::Reconnecting {
                                attempt: 0,
                                next_at: Instant::now(),
                            };
                            drop(g);
                            status.emit(
                                &config.source_id,
                                "disconnected",
                                Some("session broken".to_string()),
                                None,
                            );
                        }
                    }
                    _ = state.force_reconnect.notified() => {
                        eprintln!(
                            "[sftp] {}: force-reconnect from Ready",
                            config.source_id
                        );
                        let mut g = state.session.lock().await;
                        *g = SessionState::Reconnecting {
                            attempt: 0,
                            next_at: Instant::now(),
                        };
                        drop(g);
                        status.emit(
                            &config.source_id,
                            "reconnecting",
                            Some("user requested".to_string()),
                            None,
                        );
                    }
                    _ = state.shutdown.notified() => return,
                }
            }
            Action::BackoffWait { wait, attempt } => {
                // Wait for the backoff timer, force-reconnect, or shutdown.
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = state.force_reconnect.notified() => {
                        eprintln!(
                            "[sftp] {}: backoff skipped (user requested)",
                            config.source_id
                        );
                    }
                    _ = state.shutdown.notified() => return,
                }
                eprintln!(
                    "[sftp] {}: reconnect attempt {}",
                    config.source_id,
                    attempt + 1
                );
                match connect(&config).await {
                    Ok(conn) => {
                        let mut g = state.session.lock().await;
                        *g = SessionState::Ready {
                            conn: Arc::new(conn),
                        };
                        drop(g);
                        state.on_ready.notify_waiters();
                        status.emit(&config.source_id, "connected", None, None);
                        eprintln!("[sftp] {}: reconnected", config.source_id);
                    }
                    Err(e) => {
                        let next_attempt = attempt.saturating_add(1);
                        let next_at = Instant::now() + backoff_for(next_attempt);
                        let mut g = state.session.lock().await;
                        *g = SessionState::Reconnecting {
                            attempt: next_attempt,
                            next_at,
                        };
                        drop(g);
                        let msg = e.to_string();
                        eprintln!(
                            "[sftp] {}: reconnect failed (attempt {}): {msg}",
                            config.source_id, next_attempt
                        );
                        status.emit(
                            &config.source_id,
                            "reconnecting",
                            Some(msg),
                            Some(unix_millis_at(next_at)),
                        );
                    }
                }
            }
            Action::WaitForKick => {
                tokio::select! {
                    _ = state.force_reconnect.notified() => {
                        let mut g = state.session.lock().await;
                        *g = SessionState::Reconnecting {
                            attempt: 0,
                            next_at: Instant::now(),
                        };
                        drop(g);
                        status.emit(
                            &config.source_id,
                            "reconnecting",
                            Some("user requested".to_string()),
                            None,
                        );
                    }
                    _ = state.shutdown.notified() => return,
                }
            }
        }
    }
}

/// Exponential backoff: 1s, 2s, 4s, 8s, 16s, then capped at
/// `RECONNECT_MAX_BACKOFF` (30s).
fn backoff_for(attempt: u32) -> Duration {
    let secs = match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 16,
        _ => RECONNECT_MAX_BACKOFF.as_secs(),
    };
    Duration::from_secs(secs.min(RECONNECT_MAX_BACKOFF.as_secs()))
}

fn unix_millis_at(t: Instant) -> u64 {
    // Instant has no calendar mapping. Anchor against SystemTime::now()
    // by computing the duration to `t` and adding it to the wall clock.
    let now = Instant::now();
    let delta = t.saturating_duration_since(now);
    let wall = SystemTime::now() + delta;
    wall.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ─── Connect path ─────────────────────────────────────────────────────────────

async fn connect(config: &SftpDriverConfig) -> Result<Connection, VfsError> {
    let ssh_config = russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(60)),
        window_size: SSH_CHANNEL_WINDOW_SIZE,
        nodelay: true,
        ..Default::default()
    };
    let handler = ClientHandler {
        expected_fp: config.expected_peer_fp.clone(),
    };
    let endpoint = resolve_endpoint(&config.host, config.port, config.allow_ipv6).await?;
    let mut handle = russh::client::connect(Arc::new(ssh_config), endpoint, handler)
        .await
        .map_err(|e| VfsError::IOError(format!("ssh connect: {e}")))?;

    let seed = config.identity.signing.to_bytes();
    let private = russh::keys::ssh_key::private::Ed25519PrivateKey::from_bytes(&seed);
    let public = russh::keys::ssh_key::public::Ed25519PublicKey::from(&private);
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair { public, private };
    let priv_key: russh::keys::PrivateKey = keypair.into();

    let auth = handle
        .authenticate_publickey(
            config.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(priv_key), None),
        )
        .await
        .map_err(|e| VfsError::IOError(format!("ssh auth: {e}")))?;
    if !auth.success() {
        return Err(VfsError::PermissionDenied(
            "ssh pubkey auth rejected by peer".to_string(),
        ));
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| VfsError::IOError(format!("ssh channel: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| VfsError::IOError(format!("sftp subsystem: {e}")))?;
    let sftp_config = SftpClientConfig {
        max_packet_len: SFTP_CLIENT_MAX_PACKET_LEN,
        ..Default::default()
    };
    let sftp = SftpSession::new_with_config(channel.into_stream(), sftp_config)
        .await
        .map_err(|e| VfsError::IOError(format!("sftp init: {e}")))?;

    Ok(Connection {
        _handle: handle,
        sftp: Arc::new(sftp),
    })
}

async fn resolve_endpoint(host: &str, port: u16, allow_ipv6: bool) -> Result<SocketAddr, VfsError> {
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| VfsError::IOError(format!("resolve {host}:{port}: {e}")))?;
    addrs
        .find(|addr| allow_ipv6 || addr.is_ipv4())
        .ok_or_else(|| {
            if allow_ipv6 {
                VfsError::IOError(format!("no address found for {host}:{port}"))
            } else {
                VfsError::IOError(format!("no IPv4 address found for {host}:{port}"))
            }
        })
}

// ─── client handler ──────────────────────────────────────────────────────────

struct ClientHandler {
    expected_fp: String,
}

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        use russh::keys::ssh_key::public::KeyData;
        let raw = match server_public_key.key_data() {
            KeyData::Ed25519(ed) => ed.0,
            _ => return Ok(false),
        };
        Ok(fingerprint_of(&raw) == self.expected_fp)
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ─── Error classification ─────────────────────────────────────────────────────

/// Decide whether an SFTP error means the underlying SSH session is
/// dead. Path-level statuses (`NoSuchFile`, `PermissionDenied`) are NOT
/// fatal — the session is healthy, the operation just failed. Anything
/// transport-level (IO, EOF, decode, timeout) tears down the session.
fn is_connection_fatal(err: &russh_sftp::client::error::Error) -> bool {
    use russh_sftp::client::error::Error as E;
    matches!(
        err,
        E::IO(_) | E::Timeout | E::UnexpectedPacket | E::UnexpectedBehavior(_) | E::Limited(_)
    )
}

fn map_sftp_err(e: russh_sftp::client::error::Error) -> VfsError {
    use russh_sftp::client::error::Error as E;
    match e {
        E::Status(s) => match s.status_code {
            russh_sftp::protocol::StatusCode::NoSuchFile => VfsError::NotFound(s.error_message),
            russh_sftp::protocol::StatusCode::PermissionDenied => {
                VfsError::PermissionDenied(s.error_message)
            }
            russh_sftp::protocol::StatusCode::OpUnsupported => {
                VfsError::NotSupported(s.error_message)
            }
            _ => VfsError::IOError(format!(
                "sftp status {:?}: {}",
                s.status_code, s.error_message
            )),
        },
        other => VfsError::IOError(format!("sftp: {other}")),
    }
}

fn current_file_op_job_id() -> Option<String> {
    CURRENT_FILE_OP_JOB.with(|slot| slot.borrow().clone())
}

async fn mark_remote_transfer_context(sftp: &SftpSession, job_id: &str) {
    if !job_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return;
    }
    let path = format!("/.fbro-transfer-context/{job_id}");
    let _ = sftp
        .open_with_flags(path, russh_sftp::protocol::OpenFlags::READ)
        .await;
}

fn build_filter_regex(filter: Option<&FilterSpec>) -> Option<regex::Regex> {
    let filter = filter?;
    if filter.mode != "glob" || filter.value.trim().is_empty() {
        return None;
    }
    let regex_str = filter
        .value
        .split('|')
        .map(|part| {
            let part = part.trim();
            if part.contains('*') {
                let escaped: String =
                    part.chars()
                        .map(|ch| match ch {
                            '*' => ".*".to_string(),
                            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$'
                            | '\\' => format!("\\{ch}"),
                            _ => ch.to_string(),
                        })
                        .collect();
                format!("^{escaped}$")
            } else {
                part.chars()
                    .map(|ch| match ch {
                        '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
                        | '\\' | '*' => format!("\\{ch}"),
                        _ => ch.to_string(),
                    })
                    .collect()
            }
        })
        .collect::<Vec<String>>()
        .join("|");
    regex::RegexBuilder::new(&regex_str)
        .case_insensitive(true)
        .build()
        .ok()
}

fn sort_entries(entries: &mut [VfsEntry], sort: &SortSpec) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == VfsKind::Directory;
        let b_dir = b.kind == VfsKind::Directory;
        if a_dir != b_dir {
            return b_dir.cmp(&a_dir);
        }
        let ord = match sort.by.as_str() {
            "size" => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
            "date" => a.mtime.cmp(&b.mtime),
            "kind" => {
                let ext = |e: &VfsEntry| -> String {
                    e.name
                        .rfind('.')
                        .map(|i| e.name[i + 1..].to_lowercase())
                        .unwrap_or_default()
                };
                ext(a).cmp(&ext(b))
            }
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if sort.order == "desc" {
            ord.reverse()
        } else {
            ord
        }
    });
}

// ─── VfsDriver impl ──────────────────────────────────────────────────────────

impl VfsDriver for SftpFsDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn begin_file_op(&self, job_id: &str) {
        CURRENT_FILE_OP_JOB.with(|slot| {
            *slot.borrow_mut() = Some(job_id.to_string());
        });
    }

    fn end_file_op(&self, job_id: &str) {
        CURRENT_FILE_OP_JOB.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_deref() == Some(job_id) {
                *slot = None;
            }
        });
        self.rt().block_on(self.drop_job_connection(job_id));
    }

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let abs = self.absolute(path);
        let sort = sort.clone();
        let filter = filter.cloned();
        self.rt().block_on(async move {
            self.with_sftp(|sftp| async move {
                let dir = sftp.read_dir(&abs).await?;
                let mut out: Vec<VfsEntry> = Vec::new();
                let filter_re = build_filter_regex(filter.as_ref());
                for entry in dir {
                    let name = entry.file_name();
                    if !show_hidden && name.starts_with('.') && name != ".." {
                        continue;
                    }
                    if let Some(re) = filter_re.as_ref() {
                        if !re.is_match(&name) {
                            continue;
                        }
                    }
                    let meta = entry.metadata();
                    let kind = if meta.is_dir() {
                        VfsKind::Directory
                    } else if meta.is_symlink() {
                        VfsKind::Symlink
                    } else {
                        VfsKind::File
                    };
                    let icon = icon_name_for_entry(&name, &kind);
                    let cc = color_class_for_entry(&name, &kind);
                    let mtime = meta.mtime.and_then(|secs| {
                        format_mtime(std::time::UNIX_EPOCH + Duration::from_secs(secs as u64))
                    });
                    out.push(VfsEntry {
                        name,
                        kind,
                        size: Some(meta.len()),
                        mtime,
                        btime: None,
                        // Intentionally None: SFTP exposes Unix mode bits
                        // (`0o100644` etc.) which don't match the
                        // `rwxrwxrwx` text format `file_ops::apply_file_attrs`
                        // parses. Returning the raw octal made copies of
                        // remote files land with `mode = 0` (unreadable).
                        // Leaving permissions unset lets the destination
                        // inherit the current process's umask — same
                        // behavior as `touch` (Shift+F4).
                        permissions: None,
                        icon_name: icon,
                        cc,
                        source_path: None,
                    });
                }
                sort_entries(&mut out, &sort);
                Ok(out)
            })
            .await
        })
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        if base.is_empty() || base == "/" {
            return name.to_string();
        }
        if base.ends_with('/') {
            format!("{base}{name}")
        } else {
            format!("{base}/{name}")
        }
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        let idx = trimmed.rfind('/')?;
        Some(trimmed[..idx].to_string())
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let abs = self.absolute(path);
        let job_id = current_file_op_job_id();
        let marker_job_id = job_id.clone();
        let probe_abs = abs.clone();
        let (sftp, file_size) = self.rt().block_on(async move {
            self.with_sftp(|sftp| async move {
                use russh_sftp::protocol::OpenFlags;
                if let Some(job_id) = marker_job_id.as_deref() {
                    mark_remote_transfer_context(&sftp, job_id).await;
                }
                let file = sftp.open_with_flags(&probe_abs, OpenFlags::READ).await?;
                let size = file.metadata().await?.len();
                drop(file);
                Ok((sftp, size))
            })
            .await
        })?;
        let (tx, rx) = mpsc::channel(SFTP_READ_AHEAD_CHUNKS * 2);
        self.rt().spawn(run_read_ahead(sftp, abs, file_size, tx));
        Ok(Box::new(SftpReadStream {
            driver: SftpFsDriverRef(self as *const SftpFsDriver),
            rx,
            current: Vec::new(),
            current_pos: 0,
            done: false,
            job_id,
        }))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "remote sources are read-only in v1".to_string(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(String::new()))
    }
    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(String::new()))
    }
    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(String::new()))
    }

    fn exists(&self, path: &str) -> bool {
        let abs = self.absolute(path);
        self.rt()
            .block_on(async move {
                self.with_sftp(|sftp| async move {
                    Ok::<bool, russh_sftp::client::error::Error>(sftp.metadata(&abs).await.is_ok())
                })
                .await
            })
            .unwrap_or(false)
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        let abs = self.absolute(path);
        self.rt()
            .block_on(async move {
                self.with_sftp(|sftp| async move {
                    let meta = sftp.metadata(&abs).await?;
                    Ok(meta.len())
                })
                .await
            })
            .ok()
    }

    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
        let abs = self.absolute(path);
        self.rt().block_on(async move {
            self.with_sftp(|sftp| async move {
                let dir = sftp.read_dir(&abs).await?;
                Ok(dir
                    .into_iter()
                    .map(|e| {
                        let meta = e.metadata();
                        DirChild {
                            name: e.file_name(),
                            is_dir: meta.is_dir(),
                            size: meta.len(),
                            mtime: meta.mtime.map(|s| s as u64),
                            // See `list()` — leave permissions unset on
                            // remote sources so copies inherit the local
                            // umask rather than getting chmod'd to 0.
                            permissions: None,
                        }
                    })
                    .collect())
            })
            .await
        })
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        let abs = self.absolute(path);
        let job_id = current_file_op_job_id();
        self.rt().block_on(async move {
            self.with_sftp(|sftp| async move {
                use russh_sftp::protocol::OpenFlags;
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
                if let Some(job_id) = job_id.as_deref() {
                    mark_remote_transfer_context(&sftp, job_id).await;
                }
                let mut file = sftp.open_with_flags(&abs, OpenFlags::READ).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut buf = vec![0u8; size];
                let mut filled = 0;
                while filled < size {
                    let n = file.read(&mut buf[filled..]).await?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                buf.truncate(filled);
                drop(file);
                Ok(buf)
            })
            .await
        })
    }

    fn transfer_digest(
        &self,
        job_id: &str,
        path: &str,
    ) -> Result<Option<VfsTransferDigest>, VfsError> {
        if !job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        {
            return Err(VfsError::IOError("invalid job id".to_string()));
        }
        let abs = self.absolute(path);
        let encoded_path = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(abs.as_bytes());
        let control_path = format!("/.fbro-integrity/result/{job_id}/{encoded_path}");
        let response = self.rt().block_on(async move {
            self.with_sftp(|sftp| async move {
                use russh_sftp::protocol::OpenFlags;
                use tokio::io::AsyncReadExt;
                let mut file = sftp.open_with_flags(&control_path, OpenFlags::READ).await?;
                let mut body = String::new();
                file.read_to_string(&mut body).await?;
                drop(file);
                Ok(body)
            })
            .await
        })?;
        let parsed: RemoteDigestResponse =
            serde_json::from_str(&response).map_err(|e| VfsError::IOError(e.to_string()))?;
        if !parsed.complete {
            return Err(VfsError::IOError(format!(
                "remote {FBRO_INTEGRITY_ALGORITHM} digest is incomplete"
            )));
        }
        if parsed.algorithm != FBRO_INTEGRITY_ALGORITHM {
            return Err(VfsError::IOError(format!(
                "unsupported remote digest algorithm {}",
                parsed.algorithm
            )));
        }
        Ok(Some(VfsTransferDigest {
            algorithm: parsed.algorithm,
            size: parsed.size,
            hash: parsed.hash,
        }))
    }
}

// ─── Streaming Read adapter ──────────────────────────────────────────────────

/// Raw pointer back to the driver so `open_read` can return a sync `Read`
/// adapter without an awkward Arc-self lifetime. The driver lives in
/// `state.drivers` as `Arc<dyn VfsDriver>` for the entire mount lifetime;
/// `Box<dyn Read>` instances returned by `open_read` are consumed by a
/// single copy job and dropped well before the source is unmounted.
#[derive(Copy, Clone)]
struct SftpFsDriverRef(*const SftpFsDriver);
unsafe impl Send for SftpFsDriverRef {}

struct SftpReadStream {
    driver: SftpFsDriverRef,
    rx: mpsc::Receiver<Result<Vec<u8>, String>>,
    current: Vec<u8>,
    current_pos: usize,
    done: bool,
    job_id: Option<String>,
}

impl SftpReadStream {
    fn mark_broken(&self) {
        // SAFETY: see SftpFsDriverRef doc — driver outlives the reader.
        let driver: &SftpFsDriver = unsafe { &*self.driver.0 };
        if let Some(job_id) = self.job_id.as_deref() {
            driver.rt().block_on(driver.drop_job_connection(job_id));
        } else {
            driver.state.broken.notify_waiters();
        }
    }
}

impl Read for SftpReadStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }

        if self.current_pos >= self.current.len() {
            self.current.clear();
            self.current_pos = 0;
            // SAFETY: see SftpFsDriverRef doc — driver outlives the reader.
            let driver: &SftpFsDriver = unsafe { &*self.driver.0 };
            match driver.rt().block_on(self.rx.recv()) {
                Some(Ok(chunk)) if !chunk.is_empty() => {
                    self.current = chunk;
                }
                Some(Ok(_)) | None => {
                    self.done = true;
                    return Ok(0);
                }
                Some(Err(e)) => {
                    self.done = true;
                    self.mark_broken();
                    return Err(io::Error::new(io::ErrorKind::Other, e));
                }
            }
        }

        let available = self.current.len().saturating_sub(self.current_pos);
        let n = available.min(buf.len());
        buf[..n].copy_from_slice(&self.current[self.current_pos..self.current_pos + n]);
        self.current_pos += n;
        Ok(n)
    }
}

struct ReadWork {
    seq: u64,
    offset: u64,
    len: usize,
}

enum ReadAheadResult {
    Chunk {
        worker_idx: usize,
        seq: u64,
        data: Vec<u8>,
    },
    Error(String),
}

async fn run_read_ahead(
    sftp: Arc<SftpSession>,
    abs: String,
    file_size: u64,
    out_tx: mpsc::Sender<Result<Vec<u8>, String>>,
) {
    if file_size == 0 {
        return;
    }

    let total_chunks = (file_size + SFTP_READ_CHUNK_SIZE as u64 - 1) / SFTP_READ_CHUNK_SIZE as u64;
    let worker_count = SFTP_READ_AHEAD_CHUNKS.min(total_chunks as usize).max(1);
    let (result_tx, mut result_rx) = mpsc::channel::<ReadAheadResult>(worker_count * 2);
    let mut work_txs = Vec::with_capacity(worker_count);

    for worker_idx in 0..worker_count {
        let (work_tx, work_rx) = mpsc::channel::<ReadWork>(1);
        work_txs.push(work_tx);
        let worker_sftp = sftp.clone();
        let worker_abs = abs.clone();
        let worker_result_tx = result_tx.clone();
        tokio::spawn(async move {
            read_ahead_worker(
                worker_idx,
                worker_sftp,
                worker_abs,
                work_rx,
                worker_result_tx,
            )
            .await;
        });
    }
    drop(result_tx);

    let mut next_dispatch_seq = 0u64;
    let mut next_emit_seq = 0u64;
    let mut pending = BTreeMap::<u64, Vec<u8>>::new();

    for worker_idx in 0..worker_count {
        if next_dispatch_seq >= total_chunks {
            break;
        }
        if !dispatch_read_work(
            &work_txs[worker_idx],
            next_dispatch_seq,
            file_size,
            &mut next_dispatch_seq,
        )
        .await
        {
            let _ = out_tx
                .send(Err("read-ahead worker closed before dispatch".to_string()))
                .await;
            return;
        }
    }

    while next_emit_seq < total_chunks {
        let Some(result) = result_rx.recv().await else {
            let _ = out_tx
                .send(Err(
                    "read-ahead workers stopped before transfer completed".to_string()
                ))
                .await;
            return;
        };
        match result {
            ReadAheadResult::Chunk {
                worker_idx,
                seq,
                data,
            } => {
                pending.insert(seq, data);
                if next_dispatch_seq < total_chunks {
                    if !dispatch_read_work(
                        &work_txs[worker_idx],
                        next_dispatch_seq,
                        file_size,
                        &mut next_dispatch_seq,
                    )
                    .await
                    {
                        let _ = out_tx
                            .send(Err("read-ahead worker closed before dispatch".to_string()))
                            .await;
                        return;
                    }
                }
            }
            ReadAheadResult::Error(e) => {
                let _ = out_tx.send(Err(e)).await;
                return;
            }
        }

        while let Some(data) = pending.remove(&next_emit_seq) {
            if out_tx.send(Ok(data)).await.is_err() {
                return;
            }
            next_emit_seq += 1;
        }
    }
}

async fn dispatch_read_work(
    work_tx: &mpsc::Sender<ReadWork>,
    seq: u64,
    file_size: u64,
    next_dispatch_seq: &mut u64,
) -> bool {
    let offset = seq * SFTP_READ_CHUNK_SIZE as u64;
    let remaining = file_size.saturating_sub(offset);
    let len = remaining.min(SFTP_READ_CHUNK_SIZE as u64) as usize;
    if work_tx.send(ReadWork { seq, offset, len }).await.is_err() {
        return false;
    }
    *next_dispatch_seq += 1;
    true
}

async fn read_ahead_worker(
    worker_idx: usize,
    sftp: Arc<SftpSession>,
    abs: String,
    mut work_rx: mpsc::Receiver<ReadWork>,
    result_tx: mpsc::Sender<ReadAheadResult>,
) {
    use russh_sftp::protocol::OpenFlags;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = match sftp.open_with_flags(&abs, OpenFlags::READ).await {
        Ok(file) => file,
        Err(e) => {
            let _ = result_tx
                .send(ReadAheadResult::Error(format!("open {abs}: {e}")))
                .await;
            return;
        }
    };

    while let Some(work) = work_rx.recv().await {
        if let Err(e) = file.seek(std::io::SeekFrom::Start(work.offset)).await {
            let _ = result_tx
                .send(ReadAheadResult::Error(format!("seek {abs}: {e}")))
                .await;
            return;
        }

        let mut data = vec![0u8; work.len];
        let mut filled = 0usize;
        while filled < work.len {
            match file.read(&mut data[filled..]).await {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => {
                    let _ = result_tx
                        .send(ReadAheadResult::Error(format!("read {abs}: {e}")))
                        .await;
                    return;
                }
            }
        }
        if filled < work.len {
            let _ = result_tx
                .send(ReadAheadResult::Error(format!(
                    "read {abs}: unexpected EOF at offset {}",
                    work.offset + filled as u64
                )))
                .await;
            return;
        }
        if result_tx
            .send(ReadAheadResult::Chunk {
                worker_idx,
                seq: work.seq,
                data,
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

impl Drop for SftpReadStream {
    fn drop(&mut self) {
        self.rx.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_driver() -> SftpFsDriver {
        let id = Arc::new(Identity {
            signing: ed25519_dalek::SigningKey::from_bytes(&[0u8; 32]),
            fingerprint: "fp".to_string(),
        });
        SftpFsDriver::new(
            SftpDriverConfig {
                host: "127.0.0.1".to_string(),
                port: 7879,
                user: "fbro".to_string(),
                identity: id,
                expected_peer_fp: "abc".to_string(),
                path_prefix: "/src1".to_string(),
                source_id: "test".to_string(),
                allow_ipv6: false,
            },
            Arc::new(NoopStatusBroadcaster),
        )
        .expect("driver")
    }

    #[test]
    fn absolute_combines_prefix_and_path() {
        let driver = dummy_driver();
        assert_eq!(driver.absolute(""), "/src1");
        assert_eq!(driver.absolute("/"), "/src1");
        assert_eq!(driver.absolute("foo"), "/src1/foo");
        assert_eq!(driver.absolute("/foo/bar"), "/src1/foo/bar");
    }

    #[test]
    fn backoff_schedule_is_bounded() {
        assert_eq!(backoff_for(0).as_secs(), 1);
        assert_eq!(backoff_for(1).as_secs(), 1);
        assert_eq!(backoff_for(2).as_secs(), 2);
        assert_eq!(backoff_for(5).as_secs(), 16);
        assert_eq!(backoff_for(6).as_secs(), RECONNECT_MAX_BACKOFF.as_secs());
        assert_eq!(backoff_for(100).as_secs(), RECONNECT_MAX_BACKOFF.as_secs());
    }

    #[test]
    fn is_connection_fatal_only_for_transport_errors() {
        use russh_sftp::client::error::Error as E;
        use russh_sftp::protocol::StatusCode;
        let status = E::Status(russh_sftp::protocol::Status {
            id: 0,
            status_code: StatusCode::NoSuchFile,
            error_message: "x".to_string(),
            language_tag: "en-US".to_string(),
        });
        assert!(!is_connection_fatal(&status));
        let io = E::IO("broken pipe".to_string());
        assert!(is_connection_fatal(&io));
        assert!(is_connection_fatal(&E::Timeout));
    }

    #[test]
    fn sort_entries_matches_panel_sorting() {
        let mut entries = vec![
            test_entry("b.txt", VfsKind::File, Some(20), Some("2")),
            test_entry("dir", VfsKind::Directory, None, Some("1")),
            test_entry("a.txt", VfsKind::File, Some(10), Some("3")),
        ];
        sort_entries(
            &mut entries,
            &SortSpec {
                by: "name".to_string(),
                order: "asc".to_string(),
            },
        );
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["dir", "a.txt", "b.txt"]
        );

        sort_entries(
            &mut entries,
            &SortSpec {
                by: "size".to_string(),
                order: "desc".to_string(),
            },
        );
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["dir", "b.txt", "a.txt"]
        );
    }

    #[test]
    fn build_filter_regex_supports_globs_and_substrings() {
        let filter = FilterSpec {
            mode: "glob".to_string(),
            value: "*.jpg|notes".to_string(),
        };
        let re = build_filter_regex(Some(&filter)).expect("regex");
        assert!(re.is_match("PHOTO.JPG"));
        assert!(re.is_match("meeting-notes.txt"));
        assert!(!re.is_match("archive.zip"));
    }

    fn test_entry(name: &str, kind: VfsKind, size: Option<u64>, mtime: Option<&str>) -> VfsEntry {
        VfsEntry {
            name: name.to_string(),
            kind,
            size,
            mtime: mtime.map(str::to_string),
            btime: None,
            permissions: None,
            icon_name: None,
            cc: None,
            source_path: None,
        }
    }
}
