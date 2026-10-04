//! Short, atomic Git mutations use the Git modal's correlated activity lifecycle.
//! Closing the modal backgrounds the activity; a started switch is never aborted.
pub(crate) async fn create_branch(root: String, branch: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::create_branch(&root, &branch))
        .await
        .map_err(|e| format!("Branch creation job failed: {e}"))?
}
