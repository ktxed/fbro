//! Persistent share state at `~/.config/fbro/share.json`.
//!
//! Holds the user's last sharing toggle and selected-source set so the
//! choice survives an fbro restart. This is what makes the
//! pair-once-then-restart workflow tolerable: peer B's listener comes
//! back up automatically on next launch, and A's existing paired entry
//! is immediately usable again.
//!
//! Schema mirrors `pairing.rs` (versioned JSON, atomic temp+rename).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::{SharedRoot, SharedRootOrigin};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShareState {
    pub enabled: bool,
    pub allow_ipv6: bool,
    pub shared_source_ids: HashSet<String>,
    pub shared_roots: Vec<SharedRoot>,
}

#[derive(Serialize, Deserialize)]
struct ShareFile {
    /// Bump on incompatible schema changes; unknown versions are rejected
    /// rather than silently dropping fields.
    version: u32,
    enabled: bool,
    #[serde(default)]
    allow_ipv6: bool,
    shared_source_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ShareFileV2 {
    version: u32,
    enabled: bool,
    #[serde(default)]
    allow_ipv6: bool,
    #[serde(default)]
    shared_source_ids: Vec<String>,
    #[serde(default)]
    shared_roots: Vec<SharedRootFile>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SharedRootFile {
    id: String,
    name: String,
    path: String,
    source_id: String,
    origin: String,
    #[serde(default)]
    icon_name: Option<String>,
}

const SHARE_FILE_VERSION: u32 = 2;

pub fn default_share_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("fbro").join("share.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("fbro")
        .join("share.json")
}

pub fn load(path: &Path) -> io::Result<ShareState> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ShareState::default()),
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    if version == 1 {
        let parsed: ShareFile = serde_json::from_value(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        return Ok(ShareState {
            enabled: parsed.enabled,
            allow_ipv6: parsed.allow_ipv6,
            shared_source_ids: parsed.shared_source_ids.into_iter().collect(),
            shared_roots: Vec::new(),
        });
    }
    if version != SHARE_FILE_VERSION as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("share.json version {} not supported", version),
        ));
    }
    let parsed: ShareFileV2 = serde_json::from_value(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(ShareState {
        enabled: parsed.enabled,
        allow_ipv6: parsed.allow_ipv6,
        shared_source_ids: parsed.shared_source_ids.into_iter().collect(),
        shared_roots: parsed
            .shared_roots
            .into_iter()
            .map(|r| SharedRoot {
                id: r.id,
                name: r.name,
                path: r.path,
                source_id: r.source_id,
                origin: SharedRootOrigin::from_str(&r.origin),
                icon_name: r.icon_name,
            })
            .collect(),
    })
}

pub fn save(path: &Path, state: &ShareState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut ids: Vec<String> = state.shared_source_ids.iter().cloned().collect();
    ids.sort();
    let roots: Vec<SharedRootFile> = state
        .shared_roots
        .iter()
        .map(|r| SharedRootFile {
            id: r.id.clone(),
            name: r.name.clone(),
            path: r.path.clone(),
            source_id: r.source_id.clone(),
            origin: r.origin.as_str().to_string(),
            icon_name: r.icon_name.clone(),
        })
        .collect();
    let body = serde_json::to_vec_pretty(&ShareFileV2 {
        version: SHARE_FILE_VERSION,
        enabled: state.enabled,
        allow_ipv6: state.allow_ipv6,
        shared_source_ids: ids,
        shared_roots: roots,
    })?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fbro-share-{label}-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn missing_file_yields_default() {
        let p = tmp_path("missing");
        let _ = std::fs::remove_file(&p);
        let s = load(&p).expect("missing → default");
        assert!(!s.enabled);
        assert!(s.shared_source_ids.is_empty());
        assert!(s.shared_roots.is_empty());
    }

    #[test]
    fn roundtrip_preserves_state() {
        let p = tmp_path("roundtrip");
        let mut want = ShareState::default();
        want.enabled = true;
        want.allow_ipv6 = true;
        want.shared_source_ids.insert("vol-root".to_string());
        want.shared_source_ids.insert("vol-photos".to_string());
        want.shared_roots.push(SharedRoot {
            id: "share-home".to_string(),
            name: "Home".to_string(),
            path: "/home/me".to_string(),
            source_id: "vol-root".to_string(),
            origin: SharedRootOrigin::Home,
            icon_name: None,
        });
        save(&p, &want).expect("save");
        let got = load(&p).expect("load");
        assert_eq!(got, want);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn legacy_file_defaults_ipv6_to_disabled() {
        let p = tmp_path("legacy");
        std::fs::write(
            &p,
            br#"{"version":1,"enabled":true,"shared_source_ids":["vol-root"]}"#,
        )
        .unwrap();
        let got = load(&p).expect("load");
        assert!(got.enabled);
        assert!(!got.allow_ipv6);
        assert!(got.shared_source_ids.contains("vol-root"));
        assert!(got.shared_roots.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let p = tmp_path("corrupt");
        std::fs::write(&p, b"not json").unwrap();
        assert!(load(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unsupported_version_rejected() {
        let p = tmp_path("badver");
        std::fs::write(
            &p,
            br#"{"version":99,"enabled":true,"shared_source_ids":[]}"#,
        )
        .unwrap();
        assert!(load(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
