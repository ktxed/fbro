//! Ed25519 identity for remote-fbro.
//!
//! On first use, generate `~/.config/fbro/id_ed25519` (raw 32-byte secret
//! seed, mode 0600) and load it on subsequent runs. This single keypair is
//! used both as the local server's host key and as the client identity when
//! connecting out to a paired peer.

use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Owned Ed25519 keypair plus its public-key fingerprint.
#[derive(Clone)]
pub struct Identity {
    pub signing: SigningKey,
    pub fingerprint: String,
}

impl Identity {
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }
}

/// Default on-disk location: `$XDG_CONFIG_HOME/fbro/id_ed25519` (Linux)
/// or `~/.config/fbro/id_ed25519` (fallback).
pub fn default_key_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("fbro").join("id_ed25519");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("fbro")
        .join("id_ed25519")
}

/// Load the identity from `path`, generating + persisting a fresh one if the
/// file does not exist. The parent directory is created with mode 0700
/// (Unix) on creation. The key file itself is written 0600.
pub fn load_or_generate(path: &Path) -> io::Result<Identity> {
    match fs::read(path) {
        Ok(bytes) => parse_key_bytes(&bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "key file malformed")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let signing = generate();
            persist(&signing, path)?;
            Ok(identity_from(signing))
        }
        Err(e) => Err(e),
    }
}

fn generate() -> SigningKey {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    SigningKey::from_bytes(&seed)
}

fn persist(signing: &SigningKey, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    fs::write(path, signing.to_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn parse_key_bytes(bytes: &[u8]) -> Option<Identity> {
    if bytes.len() != 32 {
        return None;
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(bytes);
    Some(identity_from(SigningKey::from_bytes(&seed)))
}

fn identity_from(signing: SigningKey) -> Identity {
    let fingerprint = fingerprint_of(&signing.verifying_key().to_bytes());
    Identity {
        signing,
        fingerprint,
    }
}

/// SHA256(pubkey) base64-encoded (URL-safe, no padding). Stable id used as
/// the key in `paired_peers` and as the `fp=` mDNS TXT value.
pub fn fingerprint_of(pubkey_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pubkey_bytes);
    let digest = hasher.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable() {
        let pk = [7u8; 32];
        assert_eq!(fingerprint_of(&pk), fingerprint_of(&pk));
        assert_ne!(fingerprint_of(&pk), fingerprint_of(&[8u8; 32]));
    }

    #[test]
    fn load_generates_and_then_reloads() {
        let tmp = std::env::temp_dir().join(format!(
            "fbro-keys-test-{}-{}.key",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let id1 = load_or_generate(&tmp).expect("generate");
        let id2 = load_or_generate(&tmp).expect("reload");
        assert_eq!(id1.public_bytes(), id2.public_bytes());
        assert_eq!(id1.fingerprint, id2.fingerprint);
        let _ = std::fs::remove_file(&tmp);
    }
}
