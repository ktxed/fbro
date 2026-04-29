use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BookmarkEntry {
    pub name: String,
    pub path: String,
}

fn bookmarks_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        format!("{home}/.config")
    });
    PathBuf::from(base).join("fbro").join("bookmarks.toml")
}

#[derive(Serialize, Deserialize, Default)]
struct BookmarksFile {
    #[serde(default)]
    bookmarks: Vec<BookmarkEntry>,
}

pub fn load() -> Vec<BookmarkEntry> {
    let path = bookmarks_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    toml::from_str::<BookmarksFile>(&content)
        .unwrap_or_default()
        .bookmarks
}

pub fn save(entries: &[BookmarkEntry]) -> io::Result<()> {
    let path = bookmarks_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = BookmarksFile { bookmarks: entries.to_vec() };
    let content = toml::to_string(&file).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    // Atomic write: write to temp file then rename.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
