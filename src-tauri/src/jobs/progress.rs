use std::io::{self, Read};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(crate) struct ProgressReporter {
    tx: mpsc::Sender<ServerMessage>,
    job_id: String,
    total_files: usize,
    total_bytes_total: u64,
    completed_files: Arc<AtomicUsize>,
    pub(crate) total_bytes_copied: Arc<AtomicU64>,
    cancel_flag: Arc<AtomicBool>,
    /// Shared across cloned reporters / per-file ProgressReader instances.
    /// Throttles Progress emission to one message per PROGRESS_INTERVAL so a
    /// batch over thousands of small files doesn't flood the WS mpsc and
    /// drop messages via `try_send` backpressure.
    last_emit_at: Arc<Mutex<Instant>>,
}

impl ProgressReporter {
    pub(crate) fn new(
        tx: mpsc::Sender<ServerMessage>,
        job_id: String,
        total_files: usize,
        total_bytes_total: u64,
        completed_files: Arc<AtomicUsize>,
        total_bytes_copied: Arc<AtomicU64>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tx,
            job_id,
            total_files,
            total_bytes_total,
            completed_files,
            total_bytes_copied,
            cancel_flag,
            // Seed in the past so the very first emit gets through.
            last_emit_at: Arc::new(Mutex::new(Instant::now() - PROGRESS_INTERVAL)),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    /// Returns true if the caller may emit now (and bumps the gate). Returns
    /// false if a prior emit landed within `PROGRESS_INTERVAL`.
    fn try_claim_emit(&self) -> bool {
        let mut guard = self.last_emit_at.lock().unwrap();
        if guard.elapsed() < PROGRESS_INTERVAL {
            return false;
        }
        *guard = Instant::now();
        true
    }

    pub(crate) fn send_snapshot(
        &self,
        current_file: Option<String>,
        status_text: Option<String>,
        current_file_total: u64,
        current_file_copied: u64,
        completed_file: bool,
    ) {
        // Always advance the completed-files counter so the running total is
        // accurate even when this particular emit is throttled.
        let completed_files = if completed_file {
            self.completed_files.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.completed_files.load(Ordering::SeqCst)
        };

        if !self.try_claim_emit() {
            return;
        }
        let total_bytes_copied = self.total_bytes_copied.load(Ordering::SeqCst);

        send_progress_sync(
            &self.tx,
            &self.job_id,
            FileOpProgressSchema {
                total_files: self.total_files,
                completed_files,
                current_file,
                status_text,
                current_file_bytes_total: current_file_total,
                current_file_bytes_copied: current_file_copied,
                total_bytes_total: self.total_bytes_total,
                total_bytes_copied,
                current_file_percent: percent(current_file_copied, current_file_total),
                overall_percent: percent(total_bytes_copied, self.total_bytes_total),
            },
        );
    }

    pub(crate) fn total_bytes_total(&self) -> u64 {
        self.total_bytes_total
    }

    /// Replace the cumulative byte counter wholesale. Used by progress sources
    /// that report percent rather than incremental byte deltas (e.g. parsing
    /// `7z.exe -bsp1` stdout).
    pub(crate) fn set_total_bytes_copied(&self, bytes: u64) {
        self.total_bytes_copied.store(bytes, Ordering::SeqCst);
    }

    pub(crate) fn file_completed(
        &self,
        current_file: String,
        current_file_total: u64,
        current_file_copied: u64,
        status_text: Option<String>,
    ) {
        self.send_snapshot(
            Some(current_file),
            status_text,
            current_file_total,
            current_file_copied,
            true,
        );
    }
}

/// A Read adapter that streams bytes through a `ProgressReporter`, emitting a
/// Progress event at most every `PROGRESS_INTERVAL` and tripping cancellation
/// via an `io::Error` (NOT `ErrorKind::Interrupted` — `io::copy` retries that).
pub(crate) struct ProgressReader<R> {
    inner: R,
    reporter: ProgressReporter,
    current_file: String,
    current_file_total: u64,
    current_file_copied: u64,
    status_prefix: &'static str,
    last_emit: Instant,
}

impl<R: Read> ProgressReader<R> {
    pub(crate) fn new(
        inner: R,
        reporter: ProgressReporter,
        current_file: String,
        current_file_total: u64,
        status_prefix: &'static str,
    ) -> Self {
        let now = Instant::now();
        reporter.send_snapshot(
            Some(current_file.clone()),
            Some(format!("{} {}", status_prefix, current_file)),
            current_file_total,
            0,
            false,
        );
        Self {
            inner,
            reporter,
            current_file,
            current_file_total,
            current_file_copied: 0,
            status_prefix,
            last_emit: now,
        }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.reporter.is_cancelled() {
            // Must NOT use ErrorKind::Interrupted: `io::copy` (and BufReader)
            // treat that as an EINTR-style transient signal and retry the read,
            // which produces a tight spin-loop instead of unwinding.
            return Err(io::Error::new(io::ErrorKind::Other, "Cancelled"));
        }

        let n = self.inner.read(buf)?;
        if n == 0 {
            self.reporter.file_completed(
                self.current_file.clone(),
                self.current_file_total,
                self.current_file_copied,
                Some(format!("{} {}", self.status_prefix, self.current_file)),
            );
            return Ok(0);
        }

        let n_u64 = n as u64;
        self.current_file_copied += n_u64;
        self.reporter
            .total_bytes_copied
            .fetch_add(n_u64, Ordering::SeqCst);

        if self.last_emit.elapsed() >= PROGRESS_INTERVAL
            || self.current_file_copied >= self.current_file_total
        {
            self.reporter.send_snapshot(
                Some(self.current_file.clone()),
                Some(format!("{} {}", self.status_prefix, self.current_file)),
                self.current_file_total,
                self.current_file_copied,
                false,
            );
            self.last_emit = Instant::now();
        }

        Ok(n)
    }
}

pub(crate) fn percent(numerator: u64, denominator: u64) -> Option<u8> {
    if denominator == 0 {
        return None;
    }
    let pct = ((numerator.saturating_mul(100)) / denominator).min(100);
    u8::try_from(pct).ok()
}

pub(crate) fn send_progress_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    progress: FileOpProgressSchema,
) {
    if let Err(e) = tx.try_send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        files: None,
        progress: Some(progress),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    }) {
        eprintln!("[fbro] send_progress_sync({job_id}) try_send dropped: {e:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn percent_clamps_to_hundred() {
        assert_eq!(percent(15, 10), Some(100));
        assert_eq!(percent(5, 10), Some(50));
        assert_eq!(percent(0, 0), None);
    }
}
