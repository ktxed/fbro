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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::state::SharedState;
use crate::ws::protocol::{now, ServerMessage};

/// A single live PTY-backed shell session.
pub struct PtySession {
    pub session_id: u32,
    pub panel_id: String,
    /// Sender for the writer thread; dropping it terminates the writer thread.
    writer_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Master PTY handle — kept so we can resize. Wrapped in Arc/Mutex because
    /// resize is a `&self` method but we want to share ownership with the
    /// killer path during `close`.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Killer for the child process — drop or call `kill` on close.
    killer: Box<dyn ChildKiller + Send + Sync>,
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
        tx: mpsc::Sender<ServerMessage>,
        state: SharedState,
        self_arc: Arc<Self>,
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

        let mut cmd = CommandBuilder::new(default_shell());
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
        spawn_reader(session_id, reader, tx.clone());

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
        let tx_exit = tx.clone();
        let panel_id_exit = panel_id.clone();
        thread::spawn(move || {
            let _ = child.wait();

            // Remove the dead session from PtyManager so re-opening works.
            self_arc.close(session_id);

            // Update AppState and collect current GUI senders.
            let (new_version, gui_senders) = {
                let mut s = state.blocking_lock();
                if let Some(panel) = s.panels.get_mut(&panel_id_exit) {
                    panel.terminal_session = None;
                    panel.version += 1;
                }
                let v = s.panels.get(&panel_id_exit).map(|p| p.version).unwrap_or(0);
                (v, s.gui_senders.clone())
            };

            let patch = ServerMessage::StatePatch {
                timestamp: now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id_exit.clone()),
                version: new_version,
                patch: serde_json::json!({ "terminalSessionId": null }),
            };
            for sender in &gui_senders {
                let _ = sender.blocking_send(patch.clone());
            }
            let _ = tx_exit.blocking_send(patch);

            let closed = ServerMessage::TerminalClosed {
                timestamp: now(),
                session_id,
                panel_id: panel_id_exit,
            };
            for sender in &gui_senders {
                let _ = sender.blocking_send(closed.clone());
            }
            let _ = tx_exit.blocking_send(closed);
        });

        let session = PtySession {
            session_id,
            panel_id,
            writer_tx: Some(writer_tx),
            master,
            killer,
        };
        self.sessions.lock().unwrap().insert(session_id, session);
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
        let master = session.master.lock().unwrap();
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

    pub fn close(&self, sid: u32) -> Option<String> {
        let mut map = self.sessions.lock().unwrap();
        let mut session = map.remove(&sid)?;
        // Drop the writer sender first so the writer thread exits cleanly.
        session.writer_tx.take();
        let _ = session.killer.kill();
        Some(session.panel_id)
    }

    /// Close all sessions belonging to a given panel. Returns their session_ids.
    pub fn close_for_panel(&self, panel_id: &str) -> Vec<u32> {
        let mut map = self.sessions.lock().unwrap();
        let to_remove: Vec<u32> = map
            .iter()
            .filter(|(_, s)| s.panel_id == panel_id)
            .map(|(k, _)| *k)
            .collect();
        let mut closed = Vec::new();
        for sid in to_remove {
            if let Some(mut session) = map.remove(&sid) {
                session.writer_tx.take();
                let _ = session.killer.kill();
                closed.push(sid);
            }
        }
        closed
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        let mut map = self.sessions.lock().unwrap();
        for (_, mut session) in map.drain() {
            session.writer_tx.take();
            let _ = session.killer.kill();
        }
    }
}

fn spawn_reader(
    session_id: u32,
    mut reader: Box<dyn Read + Send>,
    tx: mpsc::Sender<ServerMessage>,
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
                    if tx.blocking_send(msg).is_err() {
                        break;
                    }
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
