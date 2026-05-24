//! Pairing handshake: 6-digit symmetric trust establishment.
//!
//! Two peers exchange Ed25519 pubkeys + 16-byte nonces over the
//! unauthenticated TCP pairing transport in `pair_transport.rs`. Both sides
//! derive the same 6-digit code via HKDF-SHA256
//! over the sorted pubkeys with the concatenated nonces as salt. The user
//! visually compares the codes and confirms on both sides; on dual-confirm,
//! each side stores the other's pubkey in `paired_peers`.
//!
//! A man-in-the-middle who substitutes its own keys on each leg causes the
//! two ends to derive *different* codes, so the human compare detects it.
//!
//! Persistence: `paired_peers` is mirrored to `~/.config/fbro/peers.json`
//! on every change. The handshake state machine itself is volatile (lives
//! only for the 60 s window).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::state::PairedPeer;

/// Handshake validity window — the user must confirm on both sides within
/// this much wall-clock time after pairing started or the handshake is
/// dropped on both ends. Long enough for two humans on opposite sides of a
/// room to compare codes, short enough to bound the MITM-spam attack window.
pub const HANDSHAKE_TTL: Duration = Duration::from_secs(60);

/// `info` parameter for the HKDF — versioned so future protocol changes can
/// be made without code collisions against an old pairing in progress.
const HKDF_INFO: &[u8] = b"fbro-pair-v1";

/// Compute the symmetric 6-digit verification code from both peers' pubkeys
/// and per-handshake nonces. Inputs are sorted lexicographically before
/// concatenation so both sides produce identical output regardless of who
/// initiated.
pub fn derive_code(
    pubkey_a: &[u8; 32],
    nonce_a: &[u8; 16],
    pubkey_b: &[u8; 32],
    nonce_b: &[u8; 16],
) -> String {
    let (pk_lo, pk_hi) = if pubkey_a <= pubkey_b {
        (pubkey_a, pubkey_b)
    } else {
        (pubkey_b, pubkey_a)
    };
    let (n_lo, n_hi) = if nonce_a <= nonce_b {
        (nonce_a, nonce_b)
    } else {
        (nonce_b, nonce_a)
    };

    let mut salt = [0u8; 32];
    salt[..16].copy_from_slice(n_lo);
    salt[16..].copy_from_slice(n_hi);

    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(pk_lo);
    ikm[32..].copy_from_slice(pk_hi);

    let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut out = [0u8; 4];
    hk.expand(HKDF_INFO, &mut out)
        .expect("HKDF-SHA256 with 4-byte okm cannot fail");
    let v = u32::from_be_bytes(out) % 1_000_000;
    format!("{:06}", v)
}

/// In-flight pairing handshake. Each side maintains one entry per active
/// handshake, keyed by `handshake_id` (a random UUID).
#[derive(Clone, Debug)]
pub struct Handshake {
    pub handshake_id: String,
    /// `true` if we initiated (we are "B" in the plan; A is the responder).
    pub initiator: bool,
    pub peer_name: String,
    pub peer_os: Option<String>,
    pub peer_pubkey: [u8; 32],
    pub peer_nonce: [u8; 16],
    pub our_nonce: [u8; 16],
    /// 6-digit code both sides should display.
    pub code: String,
    /// Local user's decision, if made. `None` until they click confirm/cancel.
    pub local_accepted: Option<bool>,
    /// Peer's confirmation, if received. (Carried over the pairing channel
    /// once they decide; both ends must accept for the handshake to succeed.)
    pub peer_accepted: Option<bool>,
    pub started_at: SystemTime,
}

impl Handshake {
    pub fn is_expired(&self) -> bool {
        SystemTime::now()
            .duration_since(self.started_at)
            .map(|d| d > HANDSHAKE_TTL)
            .unwrap_or(true)
    }

    pub fn expires_at_unix_millis(&self) -> u64 {
        let started = self
            .started_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        started + HANDSHAKE_TTL.as_millis() as u64
    }
}

// ─── peers.json IO ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct PeersFile {
    /// File format version. Bump on incompatible schema changes; the loader
    /// rejects unknown versions rather than silently dropping fields.
    version: u32,
    peers: Vec<PeersFileEntry>,
}

#[derive(Serialize, Deserialize)]
struct PeersFileEntry {
    fingerprint: String,
    /// Base64 (URL-safe, no-pad) of the 32-byte Ed25519 pubkey.
    pubkey_b64: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    os: Option<String>,
    /// Unix seconds.
    paired_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_sftp_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_pair_port: Option<u16>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_endpoint_seen_at: Option<u64>,
}

const PEERS_FILE_VERSION: u32 = 2;

pub fn default_peers_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("fbro").join("peers.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("fbro")
        .join("peers.json")
}

pub fn load_peers(path: &Path) -> io::Result<HashMap<String, PairedPeer>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let parsed: PeersFile = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if parsed.version != 1 && parsed.version != PEERS_FILE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("peers.json version {} not supported", parsed.version),
        ));
    }
    let mut out = HashMap::with_capacity(parsed.peers.len());
    for entry in parsed.peers {
        let pubkey = decode_pubkey(&entry.pubkey_b64).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("peer {} has malformed pubkey", entry.fingerprint),
            )
        })?;
        out.insert(
            entry.fingerprint.clone(),
            PairedPeer {
                fingerprint: entry.fingerprint,
                pubkey,
                name: entry.name,
                os: entry.os,
                paired_at: UNIX_EPOCH + Duration::from_secs(entry.paired_at),
                last_seen: entry.last_seen.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
                last_host: entry.last_host,
                last_sftp_port: entry.last_sftp_port,
                last_pair_port: entry.last_pair_port,
                last_endpoint_seen_at: entry
                    .last_endpoint_seen_at
                    .map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            },
        );
    }
    Ok(out)
}

pub fn save_peers(path: &Path, peers: &HashMap<String, PairedPeer>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut entries: Vec<PeersFileEntry> = peers
        .values()
        .map(|p| PeersFileEntry {
            fingerprint: p.fingerprint.clone(),
            pubkey_b64: encode_pubkey(&p.pubkey),
            name: p.name.clone(),
            os: p.os.clone(),
            paired_at: p
                .paired_at
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            last_seen: p
                .last_seen
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            last_host: p.last_host.clone(),
            last_sftp_port: p.last_sftp_port,
            last_pair_port: p.last_pair_port,
            last_endpoint_seen_at: p
                .last_endpoint_seen_at
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        })
        .collect();
    entries.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    let body = serde_json::to_vec_pretty(&PeersFile {
        version: PEERS_FILE_VERSION,
        peers: entries,
    })?;
    // Atomic write via temp+rename so a crash mid-write can't corrupt the file.
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

fn encode_pubkey(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_pubkey(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()
        .filter(|b| b.len() == 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_symmetric_in_role() {
        let pk_a = [1u8; 32];
        let pk_b = [2u8; 32];
        let n_a = [3u8; 16];
        let n_b = [4u8; 16];
        // Same inputs, A↔B swapped → same code.
        let from_a = derive_code(&pk_a, &n_a, &pk_b, &n_b);
        let from_b = derive_code(&pk_b, &n_b, &pk_a, &n_a);
        assert_eq!(from_a, from_b);
        assert_eq!(from_a.len(), 6);
        assert!(from_a.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn code_differs_for_different_keys() {
        let n_a = [0u8; 16];
        let n_b = [0u8; 16];
        let c1 = derive_code(&[1u8; 32], &n_a, &[2u8; 32], &n_b);
        let c2 = derive_code(&[1u8; 32], &n_a, &[3u8; 32], &n_b);
        assert_ne!(c1, c2);
    }

    #[test]
    fn code_differs_for_different_nonces() {
        let pk_a = [9u8; 32];
        let pk_b = [10u8; 32];
        let c1 = derive_code(&pk_a, &[0u8; 16], &pk_b, &[1u8; 16]);
        let c2 = derive_code(&pk_a, &[0u8; 16], &pk_b, &[2u8; 16]);
        assert_ne!(c1, c2);
    }

    #[test]
    fn peers_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "fbro-peers-test-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        let mut peers = HashMap::new();
        peers.insert(
            "fp1".to_string(),
            PairedPeer {
                fingerprint: "fp1".to_string(),
                pubkey: vec![42u8; 32],
                name: "test-host".to_string(),
                os: Some("linux".to_string()),
                paired_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                last_seen: None,
                last_host: Some("192.0.2.10".to_string()),
                last_sftp_port: Some(7879),
                last_pair_port: Some(7880),
                last_endpoint_seen_at: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_010)),
            },
        );

        save_peers(&tmp, &peers).expect("save");
        let loaded = load_peers(&tmp).expect("load");
        assert_eq!(loaded.len(), 1);
        let p = loaded.get("fp1").unwrap();
        assert_eq!(p.pubkey, vec![42u8; 32]);
        assert_eq!(p.name, "test-host");
        assert_eq!(p.os.as_deref(), Some("linux"));
        assert_eq!(p.last_host.as_deref(), Some("192.0.2.10"));
        assert_eq!(p.last_sftp_port, Some(7879));
        assert_eq!(p.last_pair_port, Some(7880));
        assert_eq!(
            p.last_endpoint_seen_at,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_010))
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn v1_file_loads_with_empty_endpoint() {
        let tmp = std::env::temp_dir().join(format!(
            "fbro-peers-v1-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &tmp,
            format!(
                r#"{{
  "version": 1,
  "peers": [{{
    "fingerprint": "fp1",
    "pubkey_b64": "{}",
    "name": "old-peer",
    "paired_at": 1700000000
  }}]
}}"#,
                encode_pubkey(&[42u8; 32])
            ),
        )
        .unwrap();

        let loaded = load_peers(&tmp).expect("load v1");
        let p = loaded.get("fp1").unwrap();
        assert_eq!(p.name, "old-peer");
        assert_eq!(p.last_host, None);
        assert_eq!(p.last_sftp_port, None);
        assert_eq!(p.last_pair_port, None);
        assert_eq!(p.last_endpoint_seen_at, None);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn missing_file_yields_empty() {
        let tmp = std::env::temp_dir().join(format!(
            "fbro-peers-missing-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&tmp); // ensure absent
        let loaded = load_peers(&tmp).expect("missing → ok");
        assert!(loaded.is_empty());
    }
}
