//! Embedded panel terminal — PTY-backed shell sessions.
//!
//! Each panel can host a terminal split. A `PtySession` owns a child process
//! (default shell) and a master PTY; one OS thread streams output bytes to the
//! WS sink as `ServerMessage::TerminalOutput` frames; another OS thread drains
//! a tokio mpsc channel of input bytes into the master writer.
//!
//! Lifetime: created by `PtyManager::open`, killed by `close` / `close_for_panel`
//! or implicitly via `Drop` on app shutdown.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::state::SharedState;
use crate::ws::protocol::{now, ServerMessage};

mod quick_editor;
pub(crate) use quick_editor::{locate_editor, search_names};

/// A single live PTY-backed shell session.
pub struct PtySession {
    pub session_id: u32,
    /// Panel id that currently owns this session. Wrapped in Arc<Mutex> so the
    /// wait-for-exit thread always sees the up-to-date owner (the session can
    /// be promoted from an embedded split to a sibling terminal tab).
    pub panel_id: Arc<Mutex<String>>,
    /// Sender for the writer thread; dropping it terminates the writer thread.
    writer_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Current GUI connection receiving PTY output. The frontend can reload
    /// while the child process remains alive, so the reader must not retain
    /// the sender belonging to the connection that originally opened it.
    output_tx: Arc<Mutex<mpsc::Sender<ServerMessage>>>,
    /// Set when output is rebound after a frontend reconnect. The next resize
    /// must provoke SIGWINCH even if the dimensions did not change, so a
    /// full-screen child redraws into the replacement xterm.
    needs_redraw: Arc<AtomicBool>,
    close_requested: Arc<AtomicBool>,
    /// Serialized OS resize work must never block the WebSocket output pump.
    resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    /// Killer for the child process — drop or call `kill` on close.
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileFingerprint {
    Present { size: u64, modified: SystemTime },
    Missing,
    Unavailable,
}

pub(crate) fn file_fingerprint(path: &str) -> FileFingerprint {
    match std::fs::metadata(path) {
        Ok(metadata) => match metadata.modified() {
            Ok(modified) => FileFingerprint::Present { size: metadata.len(), modified },
            Err(_) => FileFingerprint::Unavailable,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileFingerprint::Missing,
        Err(_) => FileFingerprint::Unavailable,
    }
}

pub(crate) fn fingerprint_changed(before: &FileFingerprint, after: &FileFingerprint) -> Option<bool> {
    match (before, after) {
        (FileFingerprint::Unavailable, _) | (_, FileFingerprint::Unavailable) => None,
        (FileFingerprint::Missing, FileFingerprint::Missing) => Some(false),
        (FileFingerprint::Present { .. }, FileFingerprint::Missing)
        | (FileFingerprint::Missing, FileFingerprint::Present { .. }) => Some(true),
        (left, right) => Some(left != right),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuickEditCompletion {
    operation: crate::journal::JournalOperation,
    path: String,
    editor: String,
    before: FileFingerprint,
}

impl QuickEditCompletion {
    pub(crate) fn new(
        operation: crate::journal::JournalOperation,
        path: String,
        editor: String,
        before: FileFingerprint,
    ) -> Self {
        Self { operation, path, editor, before }
    }
}

#[derive(Debug)]
pub enum PtyError {
    NotFound,
    Spawn(String),
    Io(String),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::NotFound => write!(f, "no such terminal session"),
            PtyError::Spawn(s) => write!(f, "failed to spawn shell: {s}"),
            PtyError::Io(s) => write!(f, "pty io error: {s}"),
        }
    }
}

impl std::error::Error for PtyError {}

/// Holds all live PTY sessions for the app, keyed by session_id.
pub struct PtyManager {
    sessions: Mutex<HashMap<u32, PtySession>>,
    next_id: AtomicU32,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        }
    }

    /// Spawn a shell rooted at `cwd`, return the new session_id.
    /// `tx` is the per-client WS sender used to push terminal output.
    /// `state` and `self_arc` are used by the wait-for-exit thread to do
    /// full cleanup (clear panel state, broadcast patch) when the shell exits naturally.
    pub fn open(
        &self,
        panel_id: String,
        cwd: &str,
        cols: u16,
        rows: u16,
        zsh_emacs_keymap: bool,
        tx: mpsc::Sender<ServerMessage>,
        state: SharedState,
        self_arc: Arc<Self>,
    ) -> Result<u32, PtyError> {
        let shell = default_shell();
        let mut args = vec!["-l".to_string()];
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if zsh_emacs_keymap
            && (shell_name.eq_ignore_ascii_case("zsh")
                || shell_name.eq_ignore_ascii_case("zsh.exe"))
        {
            args.extend(["-o".to_string(), "emacs".to_string()]);
        }
        self.open_command(
            panel_id,
            cwd,
            cols,
            rows,
            &shell,
            &args,
            tx,
            state,
            self_arc,
            None,
        )
    }

    /// Spawn a command rooted at `cwd` in an embedded panel terminal.
    /// Arguments are passed directly to the executable rather than through a
    /// shell, which keeps file paths (including spaces and shell metacharacters)
    /// safe for quick-edit sessions.
    pub fn open_command(
        &self,
        panel_id: String,
        cwd: &str,
        cols: u16,
        rows: u16,
        program: &str,
        args: &[String],
        tx: mpsc::Sender<ServerMessage>,
        state: SharedState,
        self_arc: Arc<Self>,
        quick_edit: Option<QuickEditCompletion>,
    ) -> Result<u32, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        // Drop the slave end — master keeps the pty alive.
        drop(pair.slave);

        let killer = child.clone_killer();
        let master = Arc::new(Mutex::new(pair.master));

        let session_id = self.next_id.fetch_add(1, Ordering::SeqCst);

        // Reader thread: blocking read → forward bytes via WS sender.
        let reader = master
            .lock()
            .unwrap()
            .try_clone_reader()
            .map_err(|e| PtyError::Io(e.to_string()))?;
        let output_tx = Arc::new(Mutex::new(tx.clone()));
        spawn_reader(session_id, reader, output_tx.clone());

        // Writer thread: drain mpsc → blocking write to master.
        let writer = master
            .lock()
            .unwrap()
            .take_writer()
            .map_err(|e| PtyError::Io(e.to_string()))?;
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        spawn_writer(session_id, writer, writer_rx);

        // Wait-for-exit thread: when the child exits naturally (user typed `exit`),
        // do the same full cleanup as an explicit ClosePanelTerminal intent —
        // remove session, clear panel state, broadcast StatePatch + TerminalClosed.
        let panel_id_shared = Arc::new(Mutex::new(panel_id));
        let close_requested = Arc::new(AtomicBool::new(false));
        let needs_redraw = Arc::new(AtomicBool::new(false));
        let redraw = needs_redraw.clone();
        let closing = close_requested.clone();
        let resize_tx = spawn_resize_worker(move |cols, rows| {
            if closing.load(Ordering::SeqCst) { return; }
            let master = master.lock().unwrap();
            if let Err(error) = resize_master(master.as_ref(), cols, rows, redraw.swap(false, Ordering::SeqCst)) {
                eprintln!("[terminal] resize sid={session_id}: {error}");
            }
        });
        let session = PtySession {
            session_id,
            panel_id: panel_id_shared.clone(),
            writer_tx: Some(writer_tx),
            output_tx,
            needs_redraw,
            close_requested: close_requested.clone(),
            resize_tx,
            killer,
        };
        self.sessions.lock().unwrap().insert(session_id, session);

        let tx_exit = tx.clone();
        let panel_id_exit = panel_id_shared.clone();
        thread::spawn(move || {
            let exit_result = child.wait();
            let cancelled = close_requested.load(Ordering::SeqCst);

            // Re-read owner here — a promote can have moved the session to a
            // different panel since spawn.
            let owner = panel_id_exit.lock().unwrap().clone();

            // Remove the dead session from PtyManager so re-opening works.
            self_arc.remove_exited(session_id);

            if let Some(quick_edit) = quick_edit {
                record_quick_edit_completion(&quick_edit, session_id, cancelled, exit_result);
            }

            // Update AppState. If the owner is a *terminal tab* (created by
            // promoting an embedded terminal), remove the tab entirely; the
            // panel only existed to host this PTY. Otherwise just clear the
            // embedded-terminal field on the source panel.
            let (closed_tab, new_version, gui_senders) = {
                let mut s = state.blocking_lock();
                let is_terminal_tab = s
                    .panels
                    .get(&owner)
                    .map(|p| p.tab_kind == crate::state::TabKind::Terminal)
                    .unwrap_or(false);

                if is_terminal_tab {
                    // Locate the slot and activate the left neighbor.
                    let slot_id = s.panels.get(&owner).map(|p| p.slot_id.clone());
                    if let Some(slot_id) = slot_id {
                        if let Some(slot) = s.slots.get_mut(&slot_id) {
                            if slot.tab_panel_ids.len() > 1 {
                                let idx = slot
                                    .tab_panel_ids
                                    .iter()
                                    .position(|id| id == &owner)
                                    .unwrap_or(0);
                                let new_active = if idx > 0 {
                                    slot.tab_panel_ids[idx - 1].clone()
                                } else {
                                    slot.tab_panel_ids[idx + 1].clone()
                                };
                                slot.tab_panel_ids.retain(|id| id != &owner);
                                slot.active_panel_id = new_active.clone();
                                if s.focus == owner {
                                    s.focus = new_active;
                                }
                                s.panels.remove(&owner);
                                (true, 0u64, s.gui_senders.clone())
                            } else {
                                // It's the only tab — leave the panel intact
                                // and just clear the terminal session.
                                if let Some(panel) = s.panels.get_mut(&owner) {
                                    panel.terminal_session = None;
                                    panel.version += 1;
                                }
                                let v = s.panels.get(&owner).map(|p| p.version).unwrap_or(0);
                                (false, v, s.gui_senders.clone())
                            }
                        } else {
                            (false, 0, s.gui_senders.clone())
                        }
                    } else {
                        (false, 0, s.gui_senders.clone())
                    }
                } else {
                    if let Some(panel) = s.panels.get_mut(&owner) {
                        panel.terminal_session = None;
                        panel.version += 1;
                    }
                    let v = s.panels.get(&owner).map(|p| p.version).unwrap_or(0);
                    (false, v, s.gui_senders.clone())
                }
            };

            if closed_tab {
                // Tab removal — need a fresh snapshot so the FE picks up the
                // slot membership change and the gone panel id.
                let dir_size_tasks = Vec::new();
                let snapshot = {
                    let s = state.blocking_lock();
                    crate::ws::runtime::build_state_snapshot(&s, dir_size_tasks)
                };
                for sender in &gui_senders {
                    let _ = sender.blocking_send(snapshot.clone());
                }
                let _ = tx_exit.blocking_send(snapshot);
            } else {
                let patch = ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(owner.clone()),
                    version: new_version,
                    patch: serde_json::json!({ "terminalSessionId": null }),
                };
                for sender in &gui_senders {
                    let _ = sender.blocking_send(patch.clone());
                }
                let _ = tx_exit.blocking_send(patch);
            }

            let closed = ServerMessage::TerminalClosed {
                timestamp: now(),
                session_id,
                panel_id: owner,
            };
            for sender in &gui_senders {
                let _ = sender.blocking_send(closed.clone());
            }
            let _ = tx_exit.blocking_send(closed);
        });
        Ok(session_id)
    }

    pub fn write(&self, sid: u32, bytes: &[u8]) -> Result<(), PtyError> {
        let map = self.sessions.lock().unwrap();
        let session = map.get(&sid).ok_or(PtyError::NotFound)?;
        let tx = session.writer_tx.as_ref().ok_or(PtyError::NotFound)?;
        tx.send(bytes.to_vec()).map_err(|_| PtyError::NotFound)?;
        Ok(())
    }

    pub fn resize(&self, sid: u32, cols: u16, rows: u16) -> Result<(), PtyError> {
        let map = self.sessions.lock().unwrap();
        let session = map.get(&sid).ok_or(PtyError::NotFound)?;
        session.resize_tx.send((cols.max(1), rows.max(1))).map_err(|_| PtyError::NotFound)
    }

    pub fn close(&self, sid: u32) -> Option<String> {
        let mut session = self.sessions.lock().unwrap().remove(&sid)?;
        session.close_requested.store(true, Ordering::SeqCst);
        // Drop the writer sender first so the writer thread exits cleanly.
        session.writer_tx.take();
        let pid = session.panel_id.lock().unwrap().clone();
        // Killing/dropping a ConPTY can wait for output to drain. Keep the
        // connection pumping output and release the session registry first.
        thread::spawn(move || { let _ = session.killer.kill(); });
        Some(pid)
    }

    fn remove_exited(&self, sid: u32) {
        let session = self.sessions.lock().unwrap().remove(&sid);
        if let Some(mut session) = session {
            session.writer_tx.take();
        }
    }

    /// Re-assign the owning panel of a live session. Used when promoting an
    /// embedded terminal into its own sibling tab — the PTY stays alive, but
    /// the panel id that should receive natural-exit cleanup changes.
    pub fn rebind_panel(&self, sid: u32, new_panel_id: String) {
        let map = self.sessions.lock().unwrap();
        if let Some(s) = map.get(&sid) {
            *s.panel_id.lock().unwrap() = new_panel_id;
        }
    }

    /// Route every live PTY to a newly connected GUI. PTYs outlive frontend
    /// reloads, while the per-connection server channel does not.
    pub fn rebind_output(&self, tx: mpsc::Sender<ServerMessage>) {
        let map = self.sessions.lock().unwrap();
        for session in map.values() {
            *session.output_tx.lock().unwrap() = tx.clone();
            session.needs_redraw.store(true, Ordering::SeqCst);
        }
    }

    /// Close all sessions belonging to a given panel. Returns their session_ids.
    pub fn close_for_panel(&self, panel_id: &str) -> Vec<u32> {
        let map = self.sessions.lock().unwrap();
        let to_remove: Vec<u32> = map
            .iter()
            .filter(|(_, s)| *s.panel_id.lock().unwrap() == panel_id)
            .map(|(k, _)| *k)
            .collect();
        drop(map);
        to_remove.into_iter().filter(|sid| self.close(*sid).is_some()).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct QuickEditExit {
    outcome: &'static str,
    error: Option<String>,
    exit_code: Option<u32>,
    signal: Option<String>,
}

fn quick_edit_exit(cancelled: bool, result: &std::io::Result<ExitStatus>) -> QuickEditExit {
    if cancelled {
        return QuickEditExit { outcome: "cancelled", error: None, exit_code: result.as_ref().ok().map(ExitStatus::exit_code), signal: result.as_ref().ok().and_then(|status| status.signal().map(str::to_string)) };
    }
    match result {
        Ok(status) if status.success() => QuickEditExit { outcome: "completed", error: None, exit_code: Some(status.exit_code()), signal: None },
        Ok(status) => {
            let signal = status.signal().map(str::to_string);
            let error = signal.as_ref().map(|signal| format!("Editor exited after signal {signal}.")).unwrap_or_else(|| format!("Editor exited with code {}.", status.exit_code()));
            QuickEditExit { outcome: "failed", error: Some(error), exit_code: Some(status.exit_code()), signal }
        }
        Err(error) => QuickEditExit { outcome: "failed", error: Some(format!("Could not wait for editor process: {error}")), exit_code: None, signal: None },
    }
}

fn record_quick_edit_completion(
    completion: &QuickEditCompletion,
    session_id: u32,
    cancelled: bool,
    result: std::io::Result<ExitStatus>,
) {
    let exit = quick_edit_exit(cancelled, &result);
    let after = file_fingerprint(&completion.path);
    let changed = fingerprint_changed(&completion.before, &after);
    crate::journal::record_quick_edit_result(
        &completion.operation,
        Some(&completion.path),
        Some(&completion.editor),
        Some(session_id),
        exit.outcome,
        exit.error.as_deref(),
        changed,
        exit.exit_code,
        exit.signal.as_deref(),
    );
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        let sessions = std::mem::take(self.sessions.get_mut().unwrap());
        for (_, mut session) in sessions {
            session.close_requested.store(true, Ordering::SeqCst);
            session.writer_tx.take();
            thread::spawn(move || { let _ = session.killer.kill(); });
        }
    }
}

fn resize_master(master: &dyn MasterPty, cols: u16, rows: u16, redraw: bool) -> Result<(), PtyError> {
    if redraw {
        let current = master.get_size().ok();
        if current.is_some_and(|size| size.cols == cols && size.rows == rows) {
            let redraw_rows = if rows < u16::MAX {
                rows.saturating_add(1)
            } else {
                rows.saturating_sub(1).max(1)
            };
            master
                .resize(PtySize {
                    rows: redraw_rows,
                    cols: cols.max(1),
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| PtyError::Io(e.to_string()))?;
        }
    }
    master
        .resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::Io(e.to_string()))?;
    Ok(())
}

/// One worker per PTY preserves resize ordering and coalesces queued geometry.
/// The caller never waits for an OS operation that can require output draining.
fn spawn_resize_worker(mut resize: impl FnMut(u16, u16) + Send + 'static) -> mpsc::UnboundedSender<(u16, u16)> {
    let (tx, mut rx) = mpsc::unbounded_channel::<(u16, u16)>();
    thread::spawn(move || {
        while let Some(mut size) = rx.blocking_recv() {
            while let Ok(newer) = rx.try_recv() { size = newer; }
            resize(size.0, size.1);
        }
    });
    tx
}

fn spawn_reader(
    session_id: u32,
    mut reader: Box<dyn Read + Send>,
    output_tx: Arc<Mutex<mpsc::Sender<ServerMessage>>>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let msg = ServerMessage::TerminalOutput {
                        session_id,
                        data: buf[..n].to_vec(),
                    };
                    // A failed send only means that GUI connection went away.
                    // Keep draining the PTY so a later connection can rebind it.
                    let tx = output_tx.lock().unwrap().clone();
                    let _ = tx.blocking_send(msg);
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_writer(
    _session_id: u32,
    mut writer: Box<dyn Write + Send>,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    thread::spawn(move || {
        while let Some(bytes) = rx.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });
}

#[cfg(unix)]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

#[cfg(windows)]
fn default_shell() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use portable_pty::ExitStatus;

    use super::{file_fingerprint, fingerprint_changed, quick_edit_exit, FileFingerprint, QuickEditExit};

    #[test]
    fn terminal_resize_does_not_wait_for_output_and_coalesces_pending_sizes() {
        use std::sync::mpsc as sync;
        use std::time::Duration;
        let (started_tx, started_rx) = sync::channel();
        let (release_tx, release_rx) = sync::channel();
        let (completed_tx, completed_rx) = sync::channel();
        let mut first = true;
        let tx = super::spawn_resize_worker(move |cols, rows| {
            if first {
                first = false;
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
            }
            let _ = completed_tx.send((cols, rows));
        });
        tx.send((80, 24)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // These requests must return while the OS resize is waiting for the
        // WebSocket output pump. Only the latest pending geometry is applied.
        tx.send((100, 30)).unwrap();
        tx.send((120, 40)).unwrap();
        release_tx.send(()).unwrap();
        drop(tx);
        assert_eq!(completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(), (80, 24));
        assert_eq!(completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(), (120, 40));
        assert!(completed_rx.recv_timeout(Duration::from_secs(5)).is_err());
    }

    #[test]
    fn terminal_close_releases_registry_before_blocking_process_teardown() {
        use std::sync::{Arc, Mutex, mpsc as sync};
        use std::time::Duration;
        #[derive(Debug, Clone)]
        struct BlockingKiller {
            started: sync::Sender<()>,
            release: Arc<Mutex<sync::Receiver<()>>>,
        }
        impl portable_pty::ChildKiller for BlockingKiller {
            fn kill(&mut self) -> std::io::Result<()> {
                let _ = self.started.send(());
                let _ = self.release.lock().unwrap().recv();
                Ok(())
            }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(self.clone())
            }
        }
        let manager = Arc::new(super::PtyManager::new());
        let (started, started_rx) = sync::channel();
        let (release, release_rx) = sync::channel();
        let (output, _output_rx) = tokio::sync::mpsc::channel(1);
        let (resize, _resize_rx) = tokio::sync::mpsc::unbounded_channel();
        manager.sessions.lock().unwrap().insert(1, super::PtySession {
            session_id: 1,
            panel_id: Arc::new(Mutex::new("left".into())),
            writer_tx: None,
            output_tx: Arc::new(Mutex::new(output)),
            needs_redraw: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            close_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            resize_tx: resize,
            killer: Box::new(BlockingKiller { started, release: Arc::new(Mutex::new(release_rx)) }),
        });
        let (closed, closed_rx) = sync::channel();
        let closing_manager = manager.clone();
        std::thread::spawn(move || { let _ = closed.send(closing_manager.close(1)); });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = closed_rx.recv_timeout(Duration::from_secs(5));
        let registry_free = manager.sessions.try_lock().map(|map| map.is_empty()).unwrap_or(false);
        let _ = release.send(());
        assert_eq!(result.unwrap(), Some("left".into()));
        assert!(registry_free, "process teardown held the session registry lock");
    }

    #[test]
    fn detects_quick_edit_file_changes_and_deletion() {
        let dir = std::env::temp_dir().join(format!("fbro-quick-edit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        fs::write(&path, "before").unwrap();
        let before = file_fingerprint(path.to_str().unwrap());
        assert_eq!(fingerprint_changed(&before, &file_fingerprint(path.to_str().unwrap())), Some(false));

        fs::write(&path, "after with a different size").unwrap();
        let changed = file_fingerprint(path.to_str().unwrap());
        assert_eq!(fingerprint_changed(&before, &changed), Some(true));

        fs::remove_file(&path).unwrap();
        assert_eq!(fingerprint_changed(&changed, &file_fingerprint(path.to_str().unwrap())), Some(true));
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn reports_unknown_change_when_metadata_is_unavailable() {
        assert_eq!(fingerprint_changed(&FileFingerprint::Unavailable, &FileFingerprint::Missing), None);
    }

    #[test]
    fn classifies_quick_edit_process_outcomes() {
        assert_eq!(
            quick_edit_exit(false, &Ok(ExitStatus::with_exit_code(0))),
            QuickEditExit { outcome: "completed", error: None, exit_code: Some(0), signal: None },
        );
        assert_eq!(quick_edit_exit(false, &Ok(ExitStatus::with_exit_code(7))).outcome, "failed");
        assert_eq!(quick_edit_exit(false, &Ok(ExitStatus::with_signal("TERM"))).signal.as_deref(), Some("TERM"));
        assert_eq!(quick_edit_exit(true, &Ok(ExitStatus::with_exit_code(1))).outcome, "cancelled");
        assert_eq!(quick_edit_exit(false, &Err(std::io::Error::other("wait failed"))).outcome, "failed");
    }
}
