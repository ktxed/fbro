use std::path::Path;

use serde_json::Value;

use crate::jobs::{JobKind, JobState};
use crate::platform::{EjectAction, EjectInfo, VolumeInfo};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{handle_volumes_changed, IntentContext};

use super::arg_str;

fn path_on_volume(path: &str, roots: &[String]) -> bool {
    !path.is_empty() && roots.iter().any(|root| Path::new(path).starts_with(root))
}

fn validate_action(info: &EjectInfo, action: EjectAction, expected_count: usize) -> Result<(), (&'static str, &'static str)> {
    if action == EjectAction::EjectDrive && !info.can_eject_drive {
        return Err(("EJECT_SOURCE_UNAVAILABLE", "This drive does not support safe eject"));
    }
    if action == EjectAction::EjectDrive && expected_count != info.mounted_volume_count {
        return Err(("EJECT_SOURCE_STALE", "The drive's mounted volumes changed. Review the warning and try again."));
    }
    Ok(())
}

fn affected_roots(volumes: &[VolumeInfo], source_id: &str, drive_id: &str, action: EjectAction) -> Vec<String> {
    volumes.iter()
        .filter(|volume| volume.mounted && volume.eject.as_ref().is_some_and(|info| {
            (action == EjectAction::EjectDrive && info.drive_id == drive_id) || volume.id == source_id
        }))
        .map(|volume| volume.root_path.clone())
        .filter(|root| !root.is_empty())
        .collect()
}

fn busy_file_job(state: &crate::state::AppState, roots: &[String]) -> Option<String> {
    state.job_store.jobs.values().find(|job| {
        if !matches!(job.state, JobState::Queued | JobState::Running | JobState::AwaitingDecision) {
            return false;
        }
        let JobKind::FileOp(meta) = &job.kind else { return false };
        path_on_volume(&job.source_path, roots)
            || job.dest_path.as_deref().is_some_and(|path| path_on_volume(path, roots))
            || meta.files.iter().any(|file| path_on_volume(&file.source_path, roots))
    }).map(|job| job.job_id.clone())
}

async fn failed(ctx: &IntentContext<'_>, request_id: &str, source_id: &str, code: &str, message: impl Into<String>) {
    let _ = ctx.tx.send(ServerMessage::EjectSourceFailed {
        timestamp: now(),
        request_id: request_id.into(),
        source_id: source_id.into(),
        code: code.into(),
        message: message.into(),
    }).await;
}

pub(super) async fn handle(args: &Value, ctx: &mut IntentContext<'_>) {
    let source_id = arg_str(args, "sourceId").unwrap_or("");
    let request_id = arg_str(args, "requestId").unwrap_or("");
    let Some(action) = arg_str(args, "action").and_then(EjectAction::parse) else {
        failed(ctx, request_id, source_id, "EJECT_SOURCE_INVALID", "Invalid eject action").await;
        return;
    };
    if source_id.is_empty() || request_id.is_empty() {
        failed(ctx, request_id, source_id, "EJECT_SOURCE_INVALID", "Missing source or request ID").await;
        return;
    }

    // Re-enumerate before touching a device. A new sibling mount must not be
    // ejected under an old warning that listed fewer affected volumes.
    let fresh = ctx.volume_watcher.list_volumes().await;
    let Some(volume) = fresh.iter().find(|volume| volume.id == source_id && volume.mounted) else {
        failed(ctx, request_id, source_id, "EJECT_SOURCE_STALE", "The source is no longer mounted").await;
        return;
    };
    let Some(info) = volume.eject.as_ref() else {
        failed(ctx, request_id, source_id, "EJECT_SOURCE_UNAVAILABLE", "This source is not a removable volume").await;
        return;
    };
    let expected_count = args.get("expectedMountedVolumeCount").and_then(Value::as_u64).unwrap_or(0) as usize;
    if let Err((code, message)) = validate_action(info, action, expected_count) {
        failed(ctx, request_id, source_id, code, message).await;
        return;
    }
    let Some(device_path) = volume.device_path.as_ref() else {
        failed(ctx, request_id, source_id, "EJECT_SOURCE_UNAVAILABLE", "The device could not be identified").await;
        return;
    };
    let roots = affected_roots(&fresh, source_id, &info.drive_id, action);

    {
        let mut state = ctx.state.lock().await;
        let current = state.sources.get(source_id);
        if current.is_none_or(|source| !source.mounted || source.device_path.as_deref() != Some(device_path)) {
            drop(state);
            failed(ctx, request_id, source_id, "EJECT_SOURCE_STALE", "The source changed. Try again.").await;
            return;
        }
        if state.ejecting_drives.contains(&info.drive_id) {
            drop(state);
            failed(ctx, request_id, source_id, "EJECT_SOURCE_BUSY", "An eject is already in progress for this drive").await;
            return;
        }
        if let Some(job_id) = busy_file_job(&state, &roots) {
            drop(state);
            failed(ctx, request_id, source_id, "EJECT_SOURCE_BUSY", format!("File job {job_id} is using this volume. Let it finish or cancel it before ejecting.")).await;
            return;
        }
        state.ejecting_drives.insert(info.drive_id.clone());
    }

    let result = ctx.volume_watcher.eject(device_path, action).await;
    ctx.state.lock().await.ejecting_drives.remove(&info.drive_id);
    let volumes = ctx.volume_watcher.list_volumes().await;
    if !volumes.is_empty() {
        handle_volumes_changed(volumes, ctx.state, ctx.tx, ctx.scan_handles, ctx.watcher_cmd_tx, ctx.dir_size_queue).await;
    }
    match result {
        Ok(()) => {
            let _ = ctx.tx.send(ServerMessage::EjectSourceOk {
                timestamp: now(),
                request_id: request_id.into(),
                source_id: source_id.into(),
                action: if action == EjectAction::EjectDrive { "eject-drive" } else { "unmount-volume" }.into(),
            }).await;
        }
        Err(error) => failed(ctx, request_id, source_id, "EJECT_SOURCE_FAILED", error).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{affected_roots, path_on_volume, validate_action};
    use crate::platform::{EjectAction, EjectInfo, VolumeInfo};

    fn volume(id: &str, drive: &str, root: &str) -> VolumeInfo {
        VolumeInfo {
            id: id.into(), name: id.into(), root_path: root.into(), mounted: true,
            device_path: Some(id.into()),
            eject: Some(EjectInfo { drive_id: drive.into(), drive_name: drive.into(), mounted_volume_count: 2, can_eject_drive: true }),
        }
    }

    #[test]
    fn path_match_respects_volume_boundaries() {
        let roots = vec!["/media/usb".to_string()];
        assert!(path_on_volume("/media/usb/file", &roots));
        assert!(!path_on_volume("/media/usb-backup/file", &roots));
    }

    #[test]
    fn drive_eject_affects_siblings_but_volume_unmount_does_not() {
        let volumes = vec![volume("a", "usb", "/media/a"), volume("b", "usb", "/media/b"), volume("c", "other", "/media/c")];
        assert_eq!(affected_roots(&volumes, "a", "usb", EjectAction::UnmountVolume), vec!["/media/a"]);
        assert_eq!(affected_roots(&volumes, "a", "usb", EjectAction::EjectDrive), vec!["/media/a", "/media/b"]);
    }

    #[test]
    fn changed_sibling_count_rejects_old_drive_warning() {
        let info = EjectInfo { drive_id: "usb".into(), drive_name: "USB".into(), mounted_volume_count: 2, can_eject_drive: true };
        assert!(validate_action(&info, EjectAction::EjectDrive, 1).is_err());
        assert!(validate_action(&info, EjectAction::EjectDrive, 2).is_ok());
        assert!(validate_action(&info, EjectAction::UnmountVolume, 1).is_ok());
    }
}
