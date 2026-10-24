//! Job summaries claimed with authoritative state, before GUI delivery.

#[path = "job_policy.rs"]
mod policy;
pub use policy::JobNotificationLifecycle;

use super::{NotificationInput, NotificationLevel, NotificationService};
use crate::jobs::{AppJob, FileOpType, JobKind};
use crate::state::SharedState;
use std::time::Instant;

/// Concrete execution failures bypass the OS duration threshold. Protocol
/// validation and diagnostics keep their existing in-app feedback.
pub fn is_operation_failure(message: &crate::ws::protocol::ServerMessage) -> bool {
    use crate::ws::protocol::ServerMessage;
    match message {
        ServerMessage::Error { code, .. } => matches!(
            code.as_str(),
            "remote.connect.failed"
                | "remote.retry.failed"
                | "sharing.pair.connect"
                | "sharing.server-start"
        ),
        ServerMessage::HashCheckReportFailed { .. } => true,
        _ => false,
    }
}

pub struct JobFeedback {
    pub input: NotificationInput,
    pub os: bool,
    key: String,
}

impl JobFeedback {
    pub fn publish(self, service: &NotificationService) {
        if self.os {
            service.publish_os_once(&self.key, self.input, None);
        } else {
            service.publish_once(&self.key, self.input);
        }
    }
}

pub fn terminal_feedback(
    job: &mut AppJob,
    event: &str,
    error: Option<&str>,
) -> Option<JobFeedback> {
    if !matches!(event, "Completed" | "Cancelled" | "Failed") {
        return None;
    }
    let mut failed = event == "Failed";
    if let JobKind::HashGenerateBro(meta) = &job.kind {
        failed |= meta.result.as_ref().is_some_and(|result| {
            result
                .items
                .iter()
                .any(|item| item.status == crate::ws::protocol::HashGenerateStatus::Error)
        });
    }
    let mut details = String::new();
    let subject = match &job.kind {
        JobKind::FileOp(_) if job.notification.remote_peer.is_some() => {
            "Remote transfer".to_string()
        }
        JobKind::FileOp(meta) if meta.op_type == FileOpType::Copy => "Copy".into(),
        JobKind::Search(_) => "Search".into(),
        JobKind::FedupBro(meta) => {
            if let Some(result) = &meta.result {
                failed |= !result.errors.is_empty();
                details = format!(
                    ": {} duplicate files, {} errors",
                    result.duplicate_files,
                    result.errors.len()
                );
            }
            "Deduplication".into()
        }
        JobKind::HashCheckBro(meta) => {
            if let Some(result) = &meta.result {
                use crate::ws::protocol::HashCheckStatus;
                let mismatches = result
                    .items
                    .iter()
                    .filter(|item| item.status == HashCheckStatus::Mismatched)
                    .count();
                let errors = result
                    .items
                    .iter()
                    .filter(|item| item.status == HashCheckStatus::Error)
                    .count();
                let matched = result
                    .items
                    .iter()
                    .filter(|item| item.status == HashCheckStatus::Matched)
                    .count();
                failed |= mismatches > 0 || errors > 0;
                details = format!(": {matched} matched, {mismatches} mismatches, {errors} errors");
            }
            "Hash checks".into()
        }
        JobKind::OutgoingTransfer(_) => "Remote transfer".into(),
        // Existing generation feedback reports saved partial results. Keep its
        // canonical publication rather than adding a second failure summary.
        JobKind::HashGenerateBro(meta) if failed => {
            use crate::ws::protocol::HashGenerateStatus;
            if meta.result.as_ref().is_some_and(|result| {
                result.items.iter().any(|item| {
                    matches!(
                        item.status,
                        HashGenerateStatus::Created | HashGenerateStatus::Replaced
                    )
                })
            }) {
                return None;
            }
            "Hash generation".into()
        }
        JobKind::FileOp(meta) if failed => match meta.op_type {
            FileOpType::Move => "Move".into(),
            FileOpType::Delete => "Delete".into(),
            FileOpType::Copy => unreachable!(),
        },
        JobKind::FpBro(meta) if failed => meta.bro_name.clone(),
        JobKind::SyncBro(_) if failed => "Sync".into(),
        JobKind::RemoteManifest(_) if failed => "Remote hash calculation".into(),
        _ => return None,
    };
    if !job.notification.claim_terminal() {
        return None;
    }
    let outcome = match event {
        "Failed" => "failed",
        "Cancelled" => "cancelled",
        _ if subject == "Remote transfer" => "ended",
        _ if failed => "finished with errors",
        _ => "finished",
    };
    let level = if failed {
        NotificationLevel::Error
    } else if event == "Cancelled" {
        NotificationLevel::Warning
    } else {
        NotificationLevel::Info
    };
    if event == "Failed" {
        if let Some(error) = error.or(job.error.as_deref()) {
            details = format!(": {error}");
        }
    }
    let os = job.notification.eligible_at(Instant::now(), failed);
    let mut key = match &job.kind {
        JobKind::OutgoingTransfer(meta) => {
            format!("transfer:{}:{}:{event}", meta.peer_fp, meta.transfer_key)
        }
        _ => format!("job:{}:{event}", job.job_id),
    };
    // A serving-side idle completion can be followed by more files. Its short
    // history summary must not consume the first eligible long-transfer alert.
    if matches!(job.kind, JobKind::OutgoingTransfer(_)) && !os {
        key.push_str(":short");
    }
    Some(JobFeedback {
        input: NotificationInput {
            message: format!("{subject} {outcome}{details}"),
            level: Some(level),
            channels: None,
        },
        os,
        key,
    })
}

pub async fn publish_start(state: &SharedState, job_id: &str) {
    let publication = {
        let mut guard = state.lock().await;
        let service = guard.notifications.clone();
        let transfer = guard
            .job_store
            .jobs
            .get(job_id)
            .and_then(|job| match &job.kind {
                JobKind::OutgoingTransfer(meta) => {
                    Some((meta.peer_fp.clone(), meta.transfer_key.clone()))
                }
                _ => None,
            });
        let started_at = transfer
            .map(|(peer, token)| {
                guard
                    .remote_notifications
                    .transfer_started_at(&peer, &token)
            })
            .unwrap_or_else(Instant::now);
        let Some(job) = guard.job_store.jobs.get_mut(job_id) else {
            return;
        };
        job.notification.start_at(started_at);
        if job.notification.start_published {
            return;
        }
        job.notification.start_published = true;
        let key = match &job.kind {
            JobKind::OutgoingTransfer(meta) => Some(format!(
                "transfer:{}:{}:start",
                meta.peer_fp, meta.transfer_key
            )),
            JobKind::FileOp(_) if job.notification.remote_peer.is_some() => {
                Some(format!("job:{job_id}:start"))
            }
            _ => None,
        };
        key.map(|key| (service, key))
    };
    if let Some((service, key)) = publication {
        service.publish_os_once(
            &key,
            NotificationInput::new("Remote transfer started"),
            None,
        );
    }
}

pub async fn publish_terminal(state: &SharedState, job_id: &str, event: &str, error: Option<&str>) {
    let feedback = {
        let mut guard = state.lock().await;
        let service = guard.notifications.clone();
        guard
            .job_store
            .jobs
            .get_mut(job_id)
            .and_then(|job| terminal_feedback(job, event, error))
            .map(|feedback| (service, feedback))
    };
    if let Some((service, feedback)) = feedback {
        feedback.publish(&service);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{FileOpJobMeta, HashCheckBroJobMeta, JobState};
    use crate::ws::protocol::{HashCheckBroFileSchema, HashCheckBroResultSchema, HashCheckStatus};
    use std::sync::{atomic::AtomicBool, Arc};
    use std::time::Duration;
    use tokio::sync::watch;

    fn copy_job() -> AppJob {
        AppJob {
            job_id: "copy-job".into(),
            notification: Default::default(),
            kind: JobKind::FileOp(FileOpJobMeta {
                op_type: FileOpType::Copy,
                files: vec![],
                conflict_policy: crate::jobs::ConflictPolicy::Ask,
                apply_to_all_decision: None,
            }),
            source_panel: "left".into(),
            dest_panel: Some("right".into()),
            source_path: "/source".into(),
            dest_path: Some("/destination".into()),
            state: JobState::Running,
            progress: Default::default(),
            decision: None,
            error: None,
            background: false,
            paused: false,
            handle: None,
            cancel_tx: watch::channel(false).0,
            pause_tx: watch::channel(false).0,
            skip_current_file: Arc::new(AtomicBool::new(false)),
            keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn notifications_copy_terminal_summary_is_once_and_short_jobs_keep_history() {
        let mut job = copy_job();
        job.notification.start_at(Instant::now());
        let summary = terminal_feedback(&mut job, "Completed", None).expect("copy summary");
        assert!(!summary.os);
        let service = NotificationService::default();
        summary.publish(&service);
        assert_eq!(service.snapshot().entries.len(), 1);
        service.clear();
        assert!(terminal_feedback(&mut job, "Completed", None).is_none());
        assert!(service.snapshot().entries.is_empty());
    }

    #[test]
    fn notifications_long_copy_cancellation_and_immediate_failure_are_os_eligible() {
        for event in ["Completed", "Cancelled", "Failed"] {
            let mut job = copy_job();
            if event != "Failed" {
                job.notification
                    .start_at(Instant::now() - Duration::from_secs(30));
            }
            let summary = terminal_feedback(&mut job, event, Some("disk full")).unwrap();
            assert!(summary.os, "{event}");
            assert_eq!(
                summary.input.level,
                Some(match event {
                    "Cancelled" => NotificationLevel::Warning,
                    "Failed" => NotificationLevel::Error,
                    _ => NotificationLevel::Info,
                })
            );
        }
    }

    #[test]
    fn notifications_hash_mismatches_are_one_immediate_error_summary() {
        let mut job = copy_job();
        job.kind = JobKind::HashCheckBro(HashCheckBroJobMeta {
            result: Some(HashCheckBroResultSchema {
                ignored_count: 0,
                items: vec![HashCheckBroFileSchema {
                    hash_path: "file.b3".into(),
                    counterpart_path: "file".into(),
                    status: HashCheckStatus::Mismatched,
                    expected_hash: Some("expected".into()),
                    actual_hash: Some("actual".into()),
                    error: None,
                }],
            }),
            report_saved_path: None,
            report_error: None,
        });
        let summary = terminal_feedback(&mut job, "Completed", None).unwrap();
        assert!(summary.os);
        assert_eq!(summary.input.level, Some(NotificationLevel::Error));
        assert!(summary.input.message.contains("1 mismatch"));
        assert!(terminal_feedback(&mut job, "Completed", None).is_none());
    }

    #[test]
    fn notifications_remote_copy_uses_one_transfer_summary() {
        let mut job = copy_job();
        job.notification.remote_peer = Some("peer".into());
        let summary = terminal_feedback(&mut job, "Failed", Some("connection lost")).unwrap();
        assert!(summary.input.message.starts_with("Remote transfer"));
        assert!(summary.os);
        assert!(terminal_feedback(&mut job, "Failed", None).is_none());
    }

    #[test]
    fn notifications_move_success_has_no_new_summary_but_failures_are_reported() {
        let mut job = copy_job();
        if let JobKind::FileOp(meta) = &mut job.kind {
            meta.op_type = FileOpType::Move;
        }
        assert!(terminal_feedback(&mut job, "Completed", None).is_none());
        assert!(
            terminal_feedback(&mut job, "Failed", Some("permission denied"))
                .unwrap()
                .os
        );
    }

    #[test]
    fn notifications_validation_errors_do_not_become_os_operation_failures() {
        use crate::ws::protocol::ServerMessage;
        for (code, expected) in [
            ("remote.connect.failed", true),
            ("remote.retry.failed", true),
            ("sharing.pair.connect", true),
            ("sharing.server-start", true),
            ("remote.connect.missing-peer-id", false),
            ("sharing.pair.bad-address", false),
            ("START_FILE_OP_FAILED", false),
            ("BAD_SETTING", false),
        ] {
            assert_eq!(
                is_operation_failure(&ServerMessage::Error {
                    timestamp: 1,
                    code: code.into(),
                    message: "failure".into(),
                    recoverable: true,
                    extra: None,
                }),
                expected,
                "{code}"
            );
        }
    }

    #[test]
    fn notifications_search_deduplication_and_hash_checks_use_the_execution_threshold() {
        use crate::jobs::search::{MaskMode, SearchDepth, SearchParams};
        use crate::jobs::{FedupBroJobMeta, SearchJobMeta};
        let search = || {
            JobKind::Search(SearchJobMeta {
                source_id: "search:job".into(),
                params: SearchParams {
                    paths: vec!["/source".into()],
                    depth: SearchDepth::Current,
                    file_mask: None,
                    mask_mode: MaskMode::Substring,
                    content_query: None,
                    size_op: None,
                    size_value: None,
                    size_max: None,
                    time_op: None,
                    time_value: None,
                    time_to: None,
                    time_date_field: None,
                },
            })
        };
        for long in [false, true] {
            for kind in [
                search(),
                JobKind::FedupBro(FedupBroJobMeta {
                    inputs: vec![],
                    report_path: "/report".into(),
                    result: None,
                }),
                JobKind::HashCheckBro(HashCheckBroJobMeta {
                    result: None,
                    report_saved_path: None,
                    report_error: None,
                }),
            ] {
                let mut job = copy_job();
                job.kind = kind;
                job.notification
                    .start_at(Instant::now() - Duration::from_secs(if long { 30 } else { 0 }));
                let summary = terminal_feedback(&mut job, "Completed", None).unwrap();
                assert_eq!(summary.os, long);
                assert_eq!(summary.input.level, Some(NotificationLevel::Info));
            }
        }
    }

    #[test]
    fn notifications_all_failed_hash_generation_rows_are_an_immediate_error() {
        use crate::jobs::HashGenerateBroJobMeta;
        use crate::ws::protocol::{
            HashGenerateBroFileSchema, HashGenerateBroResultSchema, HashGenerateStatus,
        };
        let mut job = copy_job();
        job.kind = JobKind::HashGenerateBro(HashGenerateBroJobMeta {
            terminal_feedback_published: false,
            result: Some(HashGenerateBroResultSchema {
                ignored_count: 0,
                items: vec![HashGenerateBroFileSchema {
                    source_path: "file".into(),
                    hash_path: "file.b3".into(),
                    status: HashGenerateStatus::Error,
                    hash: None,
                    error: Some("permission denied".into()),
                }],
            }),
        });
        let summary = terminal_feedback(&mut job, "Completed", None).unwrap();
        assert!(summary.os);
        assert_eq!(summary.input.level, Some(NotificationLevel::Error));
    }
}
