use std::path::{Component, Path, PathBuf};

use super::{NewNoteRequest, NoteError, NoteRef, NoteStore};

/// Notes as plain Markdown files in one folder.
pub(crate) struct FolderNoteStore {
    id: String,
    name: String,
    root: PathBuf,
}

impl FolderNoteStore {
    pub(crate) fn new(id: &str, name: &str, root: PathBuf) -> Self {
        Self { id: id.into(), name: name.into(), root }
    }

    fn ensure_root(&self) -> Result<(), NoteError> {
        std::fs::create_dir_all(&self.root).map_err(|error| {
            NoteError::Unavailable(format!("Cannot create {}: {error}", self.root.display()))
        })?;
        if !self.root.is_dir() {
            return Err(NoteError::Unavailable(format!("{} is not a folder.", self.root.display())));
        }
        Ok(())
    }

    /// Resolve a note id to a path inside the root, refusing anything else.
    fn path_for(&self, note_id: &str) -> Option<PathBuf> {
        let relative = Path::new(note_id);
        let plain = !note_id.is_empty()
            && relative.components().all(|part| matches!(part, Component::Normal(_)));
        plain.then(|| self.root.join(relative))
    }

    fn base_name(request: &NewNoteRequest) -> String {
        request
            .title
            .as_deref()
            .map(sanitize_title)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string())
    }
}

/// Keep a title usable as a file name on all three platforms.
fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    cleaned.trim().trim_matches('.').trim().chars().take(120).collect()
}

impl NoteStore for FolderNoteStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn browse_location(&self) -> Result<PathBuf, NoteError> {
        self.ensure_root()?;
        Ok(self.root.clone())
    }

    fn create_note(&self, request: &NewNoteRequest) -> Result<NoteRef, NoteError> {
        self.ensure_root()?;
        let base = Self::base_name(request);
        for attempt in 1..=1000 {
            let name = if attempt == 1 { format!("{base}.md") } else { format!("{base}-{attempt}.md") };
            let path = self.root.join(&name);
            // Exclusive create: an existing note is never touched.
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(NoteRef { note_id: name, local_path: path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(NoteError::Io(format!("Cannot create {}: {error}", path.display()))),
            }
        }
        Err(NoteError::Io("Too many notes with the same name.".into()))
    }

    fn contains(&self, path: &Path) -> bool {
        // Canonical on both sides: symlinked config folders and `..` must not
        // decide membership.
        match (std::fs::canonicalize(&self.root), std::fs::canonicalize(path)) {
            (Ok(root), Ok(path)) => path != root && path.starts_with(&root),
            _ => false,
        }
    }

    fn discard_empty_note(&self, note_id: &str, expected_path: &Path) -> Result<(), NoteError> {
        let path = self
            .path_for(note_id)
            .ok_or_else(|| NoteError::Io(format!("{note_id} is not a note in {}.", self.name)))?;
        let same = |a: &Path, b: &Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        if !same(&path, expected_path) {
            return Err(NoteError::Io(format!("{} is no longer in the notes folder.", expected_path.display())));
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(NoteError::Io(format!("Cannot read {}: {error}", path.display()))),
        };
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(NoteError::Io(format!("{} is not an empty note.", path.display())));
        }
        std::fs::remove_file(&path)
            .map_err(|error| NoteError::Io(format!("Cannot remove {}: {error}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("fbro-notes-{}", uuid::Uuid::new_v4())))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store(dir: &TempDir) -> FolderNoteStore {
        FolderNoteStore::new("scratch", "Scratch", dir.0.join("scratch"))
    }

    #[test]
    fn creates_the_folder_lazily_and_timestamped_empty_notes() {
        let dir = TempDir::new();
        let store = store(&dir);
        assert!(!dir.0.join("scratch").exists());
        assert_eq!(store.browse_location().unwrap(), dir.0.join("scratch"));
        let note = store.create_note(&NewNoteRequest::default()).unwrap();
        assert!(note.note_id.ends_with(".md"));
        assert_eq!(note.note_id.len(), "2026-11-02-101500.md".len());
        assert_eq!(std::fs::read(&note.local_path).unwrap(), b"");
    }

    #[test]
    fn never_overwrites_and_suffixes_collisions() {
        let dir = TempDir::new();
        let store = store(&dir);
        let request = NewNoteRequest { title: Some("ideas".into()) };
        let first = store.create_note(&request).unwrap();
        std::fs::write(&first.local_path, "keep").unwrap();
        let second = store.create_note(&request).unwrap();
        let third = store.create_note(&request).unwrap();
        assert_eq!(first.note_id, "ideas.md");
        assert_eq!(second.note_id, "ideas-2.md");
        assert_eq!(third.note_id, "ideas-3.md");
        assert_eq!(std::fs::read_to_string(&first.local_path).unwrap(), "keep");
    }

    #[test]
    fn titles_are_sanitized() {
        assert_eq!(sanitize_title(" a/b:c? "), "a-b-c-");
        assert_eq!(sanitize_title("..."), "");
        let dir = TempDir::new();
        let note = store(&dir).create_note(&NewNoteRequest { title: Some("../x".into()) }).unwrap();
        assert_eq!(note.note_id, "-x.md");
    }

    #[test]
    fn discards_only_empty_notes_inside_the_root() {
        let dir = TempDir::new();
        let store = store(&dir);
        let empty = store.create_note(&NewNoteRequest::default()).unwrap();
        let full = store.create_note(&NewNoteRequest { title: Some("full".into()) }).unwrap();
        std::fs::write(&full.local_path, "text").unwrap();
        store.discard_empty_note(&empty.note_id, &empty.local_path).unwrap();
        assert!(!empty.local_path.exists());
        assert!(store.discard_empty_note(&full.note_id, &full.local_path).is_err());
        assert!(full.local_path.exists());
        std::fs::write(dir.0.join("outside.md"), "").unwrap();
        assert!(store.discard_empty_note("../outside.md", &dir.0.join("outside.md")).is_err());
        assert!(dir.0.join("outside.md").exists());
        // Already gone is fine.
        store.discard_empty_note(&empty.note_id, &empty.local_path).unwrap();
    }

    #[test]
    fn discard_refuses_a_same_named_note_after_the_folder_changed() {
        let dir = TempDir::new();
        let old_store = store(&dir);
        let note = old_store.create_note(&NewNoteRequest { title: Some("same".into()) }).unwrap();
        // The folder setting moved to another folder holding an empty "same.md".
        let new_store = FolderNoteStore::new("scratch", "Scratch", dir.0.join("moved"));
        let other = new_store.create_note(&NewNoteRequest { title: Some("same".into()) }).unwrap();
        assert_eq!(other.note_id, note.note_id);
        assert!(new_store.discard_empty_note(&note.note_id, &note.local_path).is_err());
        assert!(other.local_path.exists());
    }

    #[test]
    fn contains_only_files_inside_the_notes_folder() {
        let dir = TempDir::new();
        let store = store(&dir);
        let note = store.create_note(&NewNoteRequest::default()).unwrap();
        assert!(store.contains(&note.local_path));
        std::fs::create_dir_all(dir.0.join("scratch/sub")).unwrap();
        std::fs::write(dir.0.join("scratch/sub/deep.md"), "").unwrap();
        assert!(store.contains(&dir.0.join("scratch/sub/deep.md")));
        assert!(!store.contains(&dir.0.join("scratch")));
        std::fs::write(dir.0.join("outside.md"), "").unwrap();
        assert!(!store.contains(&dir.0.join("outside.md")));
        assert!(!store.contains(&dir.0.join("scratch/../outside.md")));
        assert!(!store.contains(&dir.0.join("scratch/missing.md")));
    }

    #[test]
    fn notes_folder_setting_is_absolute_or_home_relative() {
        use crate::notes::normalize_notes_folder_setting as normalize;
        assert_eq!(normalize("  ", "/home/me").unwrap(), "");
        assert_eq!(normalize("~/notes", "/home/me").unwrap(), "/home/me/notes");
        assert_eq!(normalize("~", "/home/me").unwrap(), "/home/me");
        assert!(normalize("notes", "/home/me").is_err());
        let dir = TempDir::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        let file = dir.0.join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(normalize(&file.to_string_lossy(), "/home/me").is_err());
        assert!(normalize(&dir.0.join("new-folder").to_string_lossy(), "/home/me").is_ok());
    }
}
