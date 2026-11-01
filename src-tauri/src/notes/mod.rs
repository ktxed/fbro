//! Notes: a small store abstraction over "where notes live".
//!
//! Note intents (`CreateNote`, `OpenNotes`) talk to the active store only
//! through `NoteStore`. The protocol speaks in `storeId` / `noteId`, so the
//! backing mechanism (today a plain scratch folder, later perhaps an Obsidian
//! vault) can change without touching the protocol, the frontend, or the
//! built-in editor. Every store hands back a local file; the editor opens it
//! like any other file.

mod folder;

use std::path::PathBuf;

pub(crate) use folder::FolderNoteStore;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoteError {
    /// The store's location cannot be created or reached.
    Unavailable(String),
    Io(String),
}

impl NoteError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            NoteError::Unavailable(_) => "NOTES_UNAVAILABLE",
            NoteError::Io(_) => "IO",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            NoteError::Unavailable(message) | NoteError::Io(message) => message,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct NewNoteRequest {
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteRef {
    /// Store-relative identity, opaque to callers.
    pub note_id: String,
    /// The local file the built-in editor opens.
    pub local_path: PathBuf,
}

/// Synchronous; call through `spawn_blocking`.
pub(crate) trait NoteStore: Send + Sync {
    /// Stable id, also used as the pinned-location id ("scratch").
    fn id(&self) -> &str;
    /// Label for the source bar ("Scratch").
    fn display_name(&self) -> &str;
    /// Local folder a panel opens to browse the notes; created if missing.
    fn browse_location(&self) -> Result<PathBuf, NoteError>;
    /// Create an empty note and return its id and the local file to edit.
    fn create_note(&self, request: &NewNoteRequest) -> Result<NoteRef, NoteError>;
    /// Whether `path` is one of this store's notes (anywhere under its folder).
    /// Lets Edit (F4) open notes in the built-in editor.
    fn contains(&self, path: &std::path::Path) -> bool;
    /// Remove a note this store created that was never written to. The caller
    /// has already checked it is empty and unchanged; stores check again, and
    /// refuse unless `note_id` still names `expected_path` (the store's
    /// location may have changed since the note was created).
    fn discard_empty_note(&self, note_id: &str, expected_path: &std::path::Path) -> Result<(), NoteError>;
}

/// The notes folder for the `notes_folder` setting: empty means the default
/// `config_dir()/scratch`.
pub(crate) fn resolve_notes_folder(setting: &str) -> PathBuf {
    if setting.trim().is_empty() {
        crate::config::config_dir().join("scratch")
    } else {
        PathBuf::from(setting.trim())
    }
}

/// The folder store for the `notes_folder` setting.
pub(crate) fn store_for(setting: &str) -> std::sync::Arc<dyn NoteStore> {
    std::sync::Arc::new(FolderNoteStore::new("scratch", "Scratch", resolve_notes_folder(setting)))
}

/// Validate a `notes_folder` value: empty (default), or an absolute path,
/// with a leading `~` expanded to `home`. Returns the value to store.
pub(crate) fn normalize_notes_folder_setting(value: &str, home: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let expanded = if value == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(value)
    };
    if !expanded.is_absolute() {
        return Err("The notes folder must be an absolute path (or start with ~).".into());
    }
    if expanded.exists() && !expanded.is_dir() {
        return Err(format!("{} exists and is not a folder.", expanded.display()));
    }
    Ok(crate::vfs::local::normalize_path(expanded))
}
