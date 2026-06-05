use crate::jobs::viewer_prefetch::ViewerPrefetchQueue;
use crate::platform::{self, Platform};
use crate::state::SharedState;
use crate::thumb_pool::{ThumbJobKey, ThumbPool};
/// Minimal HTTP file server for the viewer and icon serving.
///
/// On Linux, WebKitGTK hands <video> URLs to GStreamer for playback.
/// GStreamer does not know about custom URI scheme handlers registered via
/// Tauri's register_uri_scheme_protocol.  It only handles well-known schemes
/// like http://.  This server runs on localhost and serves files with proper
/// HTTP range-request support so that GStreamer can buffer and seek in media
/// files.
///
/// Also serves panel icons at /__icon__/{name} so that custom URI schemes
/// (which are unreliable on WebView2/Windows) are not needed for icons.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type SharedStateSlot = Arc<Mutex<Option<SharedState>>>;
const REMOTE_THUMB_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// In-memory thumbnail LRU cache, keyed by (path, size, mtime_secs).
/// Capped by entry count; thumbnails are tens of KB each.
pub struct ThumbCache {
    inner: Mutex<ThumbCacheInner>,
    capacity: usize,
}

struct ThumbCacheInner {
    map: HashMap<ThumbKey, (Vec<u8>, u64)>, // value: (bytes, last_used_seq)
    seq: u64,
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct ThumbKey {
    path: String,
    size: u32,
    mtime: u64,
}

impl ThumbCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(ThumbCacheInner {
                map: HashMap::new(),
                seq: 0,
            }),
            capacity,
        }
    }

    fn get(&self, key: &ThumbKey) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().ok()?;
        inner.seq += 1;
        let seq = inner.seq;
        let entry = inner.map.get_mut(key)?;
        entry.1 = seq;
        Some(entry.0.clone())
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.map.clear();
        }
    }

    fn put(&self, key: ThumbKey, bytes: Vec<u8>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.seq += 1;
        let seq = inner.seq;
        inner.map.insert(key, (bytes, seq));
        if inner.map.len() > self.capacity {
            // Evict the least-recently-used entry.
            if let Some((evict_key, _)) = inner
                .map
                .iter()
                .min_by_key(|(_, v)| v.1)
                .map(|(k, v)| (k.clone(), v.1))
            {
                inner.map.remove(&evict_key);
            }
        }
    }
}

pub fn start(
    plat: Arc<dyn Platform>,
    prefetch: Arc<ViewerPrefetchQueue>,
    thumb_cache: Arc<ThumbCache>,
    thumb_pool: Arc<ThumbPool>,
    state_slot: SharedStateSlot,
) -> u16 {
    let bind = std::env::var("FBRO_FILESERVER_BIND").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let listener = TcpListener::bind(&bind).expect("file server bind");
    let port = listener.local_addr().expect("file server addr").port();
    eprintln!("[fbro fs] listening on http://{bind} (advertising port {port})");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let plat = plat.clone();
            let prefetch = Arc::clone(&prefetch);
            let thumb_cache = Arc::clone(&thumb_cache);
            let thumb_pool = Arc::clone(&thumb_pool);
            let state_slot = Arc::clone(&state_slot);
            std::thread::spawn(move || {
                let _ = handle(
                    stream,
                    &*plat,
                    &prefetch,
                    &thumb_cache,
                    &thumb_pool,
                    &state_slot,
                );
            });
        }
    });
    port
}

fn handle(
    mut stream: TcpStream,
    plat: &dyn Platform,
    prefetch: &ViewerPrefetchQueue,
    thumb_cache: &ThumbCache,
    thumb_pool: &ThumbPool,
    state_slot: &SharedStateSlot,
) -> std::io::Result<()> {
    // Read until end-of-headers (\r\n\r\n). A single read() may return only
    // part of the request — TCP can split it arbitrarily — and parsing a
    // truncated request silently drops headers like Range:, which breaks
    // <video> seeking and produces a broken-icon error in WebKitGTK.
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            break;
        }
    }
    let req = std::str::from_utf8(&buf).unwrap_or("");

    // Parse path from request line: "GET /url-encoded-path HTTP/1.1"
    let url_path = match req.lines().next().and_then(|l| l.split_whitespace().nth(1)) {
        Some(p) => p.split('?').next().unwrap_or(p).to_string(),
        None => {
            stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
    };
    let range_dbg = req
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
        .unwrap_or("(no range)");
    eprintln!("[fbro fs] {} | {}", url_path, range_dbg);

    // Icon requests: /__icon__/{name}
    if let Some(icon_name) = url_path.strip_prefix("/__icon__/") {
        let icon_name = percent_decode(icon_name);
        let (data, ct) = platform::resolve_icon(plat, &icon_name);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nCache-Control: public, max-age=31536000, immutable\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
            data.len()
        )?;
        stream.write_all(&data)?;
        return Ok(());
    }

    // Thumbnail requests: /__thumb__/{size}/{percent-encoded-host-path}
    if let Some(rest) = url_path.strip_prefix("/__thumb__/") {
        return handle_thumb(stream, thumb_cache, thumb_pool, rest);
    }

    // Remote thumbnail requests:
    // /__remote_thumb__/{sourceId}/{size}/{mtime}/{fileSize}/{remote-path}
    if let Some(rest) = url_path.strip_prefix("/__remote_thumb__/") {
        return handle_remote_thumb(stream, state_slot, rest);
    }

    let decoded = percent_decode(&url_path);
    // On Windows the URL path has a leading / before the drive letter: /C:/... → C:/...
    #[cfg(windows)]
    let path = decoded.trim_start_matches('/').to_string();
    #[cfg(not(windows))]
    let path = decoded;

    // Parse optional Range header (case-insensitive)
    let range_str = req
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|v| v.trim().to_string());

    let total = match std::fs::metadata(&path).map(|m| m.len()) {
        Ok(n) => n,
        Err(_) => {
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
    };
    let mime = mime_for_path(&path);

    if let Some(ref rstr) = range_str {
        if let Some((start, end)) = parse_range(rstr, total) {
            let want = (end - start + 1) as usize;
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(start))?;
            let mut data = vec![0u8; want];
            let n = file.read(&mut data)?;
            data.truncate(n);
            let actual_end = start + n as u64 - 1;
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\n\
                 Content-Type: {mime}\r\n\
                 Content-Range: bytes {start}-{actual_end}/{total}\r\n\
                 Content-Length: {n}\r\n\
                 Accept-Ranges: bytes\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Cache-Control: no-store\r\n\
                 Connection: close\r\n\r\n"
            )?;
            stream.write_all(&data)?;
            return Ok(());
        }
    }

    // Full file (for images or non-range requests).
    // Check the BE prefetch cache first; fall back to disk on miss.
    // The prefetch only caches a prefix of large files (e.g. videos), so we
    // must verify the cached content is the complete file before using it —
    // otherwise we'd serve a truncated MP4 and qtdemux would reject it.
    let data = match prefetch.get_cached_by_path(&path) {
        Some(entry) if entry.file_size == Some(entry.content.len() as u64) => entry.content.clone(),
        _ => std::fs::read(&path)?,
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        data.len()
    )?;
    stream.write_all(&data)?;
    Ok(())
}

fn handle_remote_thumb(
    mut stream: TcpStream,
    state_slot: &SharedStateSlot,
    rest: &str,
) -> std::io::Result<()> {
    let mut parts = rest.splitn(5, '/');
    let Some(source_part) = parts.next() else {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let Some(size_part) = parts.next() else {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let Some(mtime_part) = parts.next() else {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let Some(file_size_part) = parts.next() else {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let Some(path_part) = parts.next() else {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let source_id = percent_decode(source_part);
    let size = size_part.parse::<u32>().unwrap_or(128).clamp(32, 512);
    let mtime = percent_decode(mtime_part);
    let file_size = percent_decode(file_size_part);
    let path = percent_decode(path_part);
    if source_id.is_empty() || path.is_empty() || path.split('/').any(|part| part == "..") {
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    }

    if let Some(cached) = read_remote_thumb_cache(&source_id, &path, &mtime, &file_size, size) {
        return write_remote_thumb_response(&mut stream, &cached);
    }

    let state = match state_slot.lock().ok().and_then(|slot| slot.clone()) {
        Some(state) => state,
        None => {
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
    };
    let driver = futures::executor::block_on(async {
        let guard = state.lock().await;
        guard.drivers.get(&source_id).cloned()
    });
    let Some(driver) = driver else {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let bytes = match driver
        .as_any()
        .downcast_ref::<crate::vfs::sftp::SftpFsDriver>()
        .and_then(|sftp| sftp.remote_thumbnail(&path, size).ok())
    {
        Some(bytes) => bytes,
        None => {
            stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )?;
            return Ok(());
        }
    };
    write_remote_thumb_cache(&source_id, &path, &mtime, &file_size, size, &bytes);
    write_remote_thumb_response(&mut stream, &bytes)
}

fn remote_thumb_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .or_else(|| Some(std::env::temp_dir()))?;
    Some(base.join("fbro").join("remote-thumbnails"))
}

fn remote_thumb_cache_path(
    source_id: &str,
    path: &str,
    mtime: &str,
    file_size: &str,
    thumb_size: u32,
) -> Option<PathBuf> {
    let mut h = blake3::Hasher::new();
    h.update(source_id.as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(mtime.as_bytes());
    h.update(b"\0");
    h.update(file_size.as_bytes());
    h.update(b"\0");
    h.update(&thumb_size.to_le_bytes());
    Some(remote_thumb_cache_dir()?.join(format!("{}.png", h.finalize().to_hex())))
}

fn read_remote_thumb_cache(
    source_id: &str,
    path: &str,
    mtime: &str,
    file_size: &str,
    thumb_size: u32,
) -> Option<Vec<u8>> {
    let cache_path = remote_thumb_cache_path(source_id, path, mtime, file_size, thumb_size)?;
    let meta = std::fs::metadata(&cache_path).ok()?;
    let fresh = meta
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .map(|age| age <= REMOTE_THUMB_TTL)
        .unwrap_or(false);
    if !fresh {
        let _ = std::fs::remove_file(&cache_path);
        return None;
    }
    std::fs::read(cache_path).ok()
}

fn write_remote_thumb_cache(
    source_id: &str,
    path: &str,
    mtime: &str,
    file_size: &str,
    thumb_size: u32,
    bytes: &[u8],
) {
    let Some(cache_path) = remote_thumb_cache_path(source_id, path, mtime, file_size, thumb_size)
    else {
        return;
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = cache_path.with_extension("png.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(tmp, cache_path);
    }
}

fn handle_thumb(
    mut stream: TcpStream,
    cache: &ThumbCache,
    pool: &ThumbPool,
    rest: &str,
) -> std::io::Result<()> {
    // Parse {size}/{rest-as-path}
    let (size_str, path_part) = match rest.split_once('/') {
        Some((a, b)) => (a, b),
        None => {
            stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
    };
    let size: u32 = size_str.parse().unwrap_or(128).clamp(32, 512);

    let decoded = percent_decode(path_part);
    #[cfg(windows)]
    let path = {
        // On Windows the path includes a leading drive letter; URL might not have leading slash.
        decoded.trim_start_matches('/').to_string()
    };
    #[cfg(not(windows))]
    let path = if decoded.starts_with('/') {
        decoded
    } else {
        format!("/{decoded}")
    };

    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let key = ThumbKey {
        path: path.clone(),
        size,
        mtime,
    };

    if let Some(cached) = cache.get(&key) {
        return write_thumb_response(&mut stream, &cached);
    }

    // Submit to the worker pool. Multiple concurrent requests for the same
    // (path, size, mtime) dedupe onto a single generation, so refreshing the
    // panel doesn't multiply work.
    let rx = pool.request(ThumbJobKey {
        path: path.clone(),
        size,
        mtime,
    });
    let bytes_opt = rx.recv().unwrap_or(None);

    match bytes_opt {
        Some(bytes) => {
            cache.put(key, bytes.clone());
            write_thumb_response(&mut stream, &bytes)
        }
        None => {
            stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )?;
            Ok(())
        }
    }
}

fn write_thumb_response(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nCache-Control: public, max-age=3600\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(bytes)?;
    Ok(())
}

fn write_remote_thumb_response(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nCache-Control: public, max-age=86400\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(bytes)?;
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let s = header.strip_prefix("bytes=")?;
    let (start_str, end_str) = s.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

fn mime_for_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}
