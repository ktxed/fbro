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
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use crate::platform::{self, Platform};

pub fn start(plat: Arc<dyn Platform>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("file server bind");
    let port = listener.local_addr().expect("file server addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let plat = plat.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &*plat);
            });
        }
    });
    port
}

fn handle(mut stream: TcpStream, plat: &dyn Platform) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let req = std::str::from_utf8(&buf[..n]).unwrap_or("");

    // Parse path from request line: "GET /url-encoded-path HTTP/1.1"
    let url_path = match req.lines().next().and_then(|l| l.split_whitespace().nth(1)) {
        Some(p) => p.to_string(),
        None => {
            stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
    };

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
                 Connection: close\r\n\r\n"
            )?;
            stream.write_all(&data)?;
            return Ok(());
        }
    }

    // Full file (for images or non-range requests)
    let data = std::fs::read(&path)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {}\r\n\
         Accept-Ranges: bytes\r\n\
         Connection: close\r\n\r\n",
        data.len()
    )?;
    stream.write_all(&data)?;
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
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
