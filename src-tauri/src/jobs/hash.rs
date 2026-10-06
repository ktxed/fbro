//! Bounded-memory BLAKE3 hashing shared by hash Bros.
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const HASH_BUFFER_SIZE: usize = 1024 * 1024;

pub(crate) fn hash_reader(
    mut reader: impl Read,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<String, String> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; HASH_BUFFER_SIZE];
    let mut count = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".into());
        }
        let n = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|e| format!("Cannot read file: {e}"))?,
        };
        if n == 0 {
            break;
        }
        hasher.update_rayon(&buffer[..n]);
        count = count.saturating_add(n as u64);
        progress(count);
    }
    progress(count);
    Ok(hasher.finalize().to_hex().to_string())
}
