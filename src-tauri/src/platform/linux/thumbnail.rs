//! Custom Linux thumbnail generator.
//!
//! Tumbler / freedesktop DBus thumbnailers are not always installed (e.g.
//! PopOS Cosmic ships none by default). Rather than depend on an external
//! service, fbro generates thumbnails itself for a curated set of formats.
//!
//! Two-level caching:
//!   L1 — `ThumbCache` in `fileserver.rs` (in-memory, shared across platforms).
//!   L2 — `~/.cache/fbro/thumbnails/<hash>.png` (this file).
//!
//! The L2 key folds canonical path + mtime + size, so an mtime bump
//! invalidates implicitly. Decode failures are recorded as empty `.fail`
//! sentinels so we don't repeatedly retry expensive work.

use std::fs;
use std::io::Cursor;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::ImageFormat;

const MAX_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Image,
    Video,
    Unknown,
}

fn classify(path: &Path) -> Kind {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return Kind::Unknown;
    };
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" => Kind::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
        _ => Kind::Unknown,
    }
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("fbro").join("thumbnails"))
}

fn mtime_ns(meta: &fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

fn cache_key(canonical: &Path, mtime: i128, size: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(canonical.as_os_str().as_bytes());
    h.update(&mtime.to_le_bytes());
    h.update(&size.to_le_bytes());
    h.finalize().to_hex().to_string()
}

fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = target.with_extension("png.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, target)
}

pub fn generate(path: &str, size: u32) -> Option<(Vec<u8>, &'static str)> {
    let p = Path::new(path);
    let kind = classify(p);
    if matches!(kind, Kind::Unknown) {
        return None;
    }

    let canonical = fs::canonicalize(p).ok()?;
    let meta = fs::metadata(&canonical).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = mtime_ns(&meta);

    let dir = cache_dir();
    let key = cache_key(&canonical, mtime, size);
    let png_path = dir.as_ref().map(|d| d.join(format!("{}.png", key)));
    let fail_path = dir.as_ref().map(|d| d.join(format!("{}.fail", key)));

    if let Some(pp) = png_path.as_ref() {
        if let Ok(bytes) = fs::read(pp) {
            return Some((bytes, "image/png"));
        }
    }
    if let Some(fp) = fail_path.as_ref() {
        if let Ok(meta) = fs::metadata(fp) {
            // Honor the sentinel for 24h. After that, retry — long enough to
            // avoid pounding genuinely-bad files, short enough that a code or
            // build-feature change recovers without manual cache cleanup.
            let stale = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|e| e.as_secs() > 24 * 3600)
                .unwrap_or(true);
            if !stale {
                return None;
            }
            let _ = fs::remove_file(fp);
        }
    }

    let result = match kind {
        Kind::Image => generate_image(&canonical, &meta, size),
        Kind::Video => generate_video(&canonical, size),
        Kind::Unknown => None,
    };

    if let Some(d) = dir.as_ref() {
        let _ = fs::create_dir_all(d);
    }
    match &result {
        Some(bytes) => {
            if let Some(pp) = png_path.as_ref() {
                let _ = atomic_write(pp, bytes);
            }
        }
        None => {
            if let Some(fp) = fail_path.as_ref() {
                let _ = fs::File::create(fp);
            }
        }
    }
    result.map(|b| (b, "image/png"))
}

fn generate_image(path: &Path, meta: &fs::Metadata, size: u32) -> Option<Vec<u8>> {
    if meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let img = image::open(path).ok()?;
    let thumb = img.thumbnail(size, size);
    let mut buf = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .ok()?;
    Some(buf)
}

#[cfg(feature = "linux-video")]
fn generate_video(path: &Path, size: u32) -> Option<Vec<u8>> {
    use ffmpeg_next as ffmpeg;
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
    });

    let mut ictx = ffmpeg::format::input(&path).ok()?;
    let stream = ictx.streams().best(ffmpeg::media::Type::Video)?;
    let stream_index = stream.index();
    let codec_params = stream.parameters();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(codec_params)
        .ok()?
        .decoder()
        .video()
        .ok()?;

    let mut frames_seen = 0u32;
    for (s, packet) in ictx.packets() {
        if s.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut frame = ffmpeg::frame::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            frames_seen += 1;
            // Skip first few frames — many videos start on black/credits.
            if frames_seen < 5 {
                continue;
            }
            let src_w = frame.width();
            let src_h = frame.height();
            if src_w == 0 || src_h == 0 {
                continue;
            }
            let scale = (size as f32 / src_w.max(src_h) as f32).min(1.0);
            let out_w = ((src_w as f32 * scale).round() as u32).max(1);
            let out_h = ((src_h as f32 * scale).round() as u32).max(1);
            let mut sc = ffmpeg::software::scaling::Context::get(
                frame.format(),
                src_w,
                src_h,
                ffmpeg::format::Pixel::RGBA,
                out_w,
                out_h,
                ffmpeg::software::scaling::Flags::BILINEAR,
            )
            .ok()?;
            let mut rgba = ffmpeg::frame::Video::empty();
            sc.run(&frame, &mut rgba).ok()?;
            let stride = rgba.stride(0);
            let data = rgba.data(0);
            let row_bytes = (out_w * 4) as usize;
            let mut tight = Vec::with_capacity(row_bytes * out_h as usize);
            for y in 0..out_h as usize {
                let off = y * stride;
                tight.extend_from_slice(&data[off..off + row_bytes]);
            }
            let img = image::RgbaImage::from_raw(out_w, out_h, tight)?;
            let mut buf = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .ok()?;
            return Some(buf);
        }
    }
    None
}

#[cfg(not(feature = "linux-video"))]
fn generate_video(_path: &Path, _size: u32) -> Option<Vec<u8>> {
    None
}
