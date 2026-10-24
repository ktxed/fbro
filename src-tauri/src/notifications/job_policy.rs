//! Monotonic execution timing and publication claims, independent of job I/O.

use std::time::{Duration, Instant};

#[derive(Default)]
pub struct JobNotificationLifecycle {
    pub(crate) started_at: Option<Instant>,
    pub(crate) start_published: bool,
    pub(crate) terminal_published: bool,
    pub(crate) remote_peer: Option<String>,
}

impl JobNotificationLifecycle {
    pub fn start_at(&mut self, now: Instant) {
        self.started_at.get_or_insert(now);
    }

    pub fn eligible_at(&self, now: Instant, failed: bool) -> bool {
        failed
            || self.started_at.is_some_and(|start| {
                now.saturating_duration_since(start) >= Duration::from_secs(30)
            })
    }

    pub fn claim_terminal(&mut self) -> bool {
        !std::mem::replace(&mut self.terminal_published, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_and_cancelled_jobs_require_thirty_seconds_of_execution() {
        let start = Instant::now();
        let mut lifecycle = JobNotificationLifecycle::default();
        lifecycle.start_at(start);
        assert!(!lifecycle.eligible_at(start + Duration::from_millis(29_999), false));
        assert!(lifecycle.eligible_at(start + Duration::from_secs(30), false));
    }

    #[test]
    fn queue_time_is_excluded_and_pause_or_resume_does_not_restart_execution_clock() {
        let queued = Instant::now();
        let start = queued + Duration::from_secs(90);
        let mut lifecycle = JobNotificationLifecycle::default();
        assert!(!lifecycle.eligible_at(start, false));
        lifecycle.start_at(start);
        lifecycle.start_at(start + Duration::from_secs(29));
        assert!(!lifecycle.eligible_at(start + Duration::from_secs(29), false));
        assert!(lifecycle.eligible_at(start + Duration::from_secs(30), false));
    }

    #[test]
    fn failure_before_execution_is_immediately_eligible() {
        assert!(JobNotificationLifecycle::default().eligible_at(Instant::now(), true));
    }

    #[test]
    fn terminal_summary_claim_survives_repeated_delivery() {
        let mut lifecycle = JobNotificationLifecycle::default();
        assert!(lifecycle.claim_terminal());
        assert!(!lifecycle.claim_terminal());
    }
}
