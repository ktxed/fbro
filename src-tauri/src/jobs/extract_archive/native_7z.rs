//! Shell out to a 7-Zip CLI binary (`7z` / `7zz` / `7za`) for `.7z` extraction.
//!
//! Native 7-Zip has a much faster LZMA decoder than what we can do in pure
//! Rust on single-block fully-solid archives, because it implements
//! speculative dictionary-state restart that lzma-rust2 doesn't. Whenever a
//! binary is available we'd rather delegate.
//!
//! This module is invoked from `extract_blocking` and reports progress back
//! through the same `ProgressReporter` the in-process path uses, so the FE
//! Operation modal doesn't care which extractor ran.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::jobs::progress::ProgressReporter;
use crate::vfs::VfsError;

pub enum NativeError {
    /// The native binary isn't usable for *this* extraction (target layout
    /// doesn't fit, listfile write failed early, etc.) — the caller should
    /// fall back to the in-process extractor.
    Unsupported(String),
    /// Hard error reported by the native process (or by us setting it up).
    Fatal(VfsError),
}

pub fn extract_via_native_7z(
    binary: &Path,
    archive_path: &Path,
    targets: &[(String, PathBuf)],
    reporter: &ProgressReporter,
    cancel_flag: &AtomicBool,
) -> Result<(), NativeError> {
    if targets.is_empty() {
        return Ok(());
    }

    // 7-Zip always writes entries at <output_dir>/<inner_archive_path>. We can
    // only delegate when every requested target dest equals
    // `<some common root>/<inner>`. That's the case when the user is browsing
    // at the archive root; if they're inside a sub-folder we strip a prefix
    // off the inner path in the dispatcher, which breaks the layout 7-Zip can
    // produce. In that case fall back to the in-process extractor.
    let dest_root = match derive_dest_root(targets) {
        Some(r) => r,
        None => {
            return Err(NativeError::Unsupported(
                "target layout doesn't match the archive layout (panel.path is not the archive root)"
                    .to_string(),
            ));
        }
    };

    std::fs::create_dir_all(&dest_root).map_err(|e| {
        NativeError::Fatal(VfsError::IOError(format!(
            "cannot create {}: {e}",
            dest_root.display()
        )))
    })?;

    // 7-Zip accepts a `@listfile` argument to scope extraction to specific
    // inner paths. Avoids blowing the command-line length limit when the
    // selection has tens of thousands of files.
    let listfile_path = std::env::temp_dir().join(format!(
        "fbro-7z-{}-{}.lst",
        std::process::id(),
        rand_suffix()
    ));
    write_listfile(&listfile_path, targets).map_err(NativeError::Fatal)?;

    eprintln!(
        "[fbro] native 7z: {} -> {} ({} targets)",
        binary.display(),
        dest_root.display(),
        targets.len()
    );

    let mut cmd = Command::new(binary);
    cmd.arg("x")
        .arg(archive_path)
        .arg(format!("-o{}", dest_root.display()))
        .arg(format!("@{}", listfile_path.display()))
        .arg("-bsp1") // progress to stdout
        .arg("-bso0") // suppress regular output
        .arg("-bse2") // errors to stderr
        .arg("-y") // assume Yes on overwrite
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&listfile_path);
            return Err(NativeError::Fatal(VfsError::IOError(format!(
                "failed to launch {}: {e}",
                binary.display()
            ))));
        }
    };

    let stdout = child.stdout.take().expect("piped above");
    let stderr = child.stderr.take().expect("piped above");
    let total_bytes = reporter.total_bytes_total();

    // std::thread::scope lets the worker threads borrow `reporter` and
    // `cancel_flag` from the calling stack without juggling Arcs.
    let result = std::thread::scope(|scope| -> Result<(), NativeError> {
        let stdout_handle = scope.spawn(|| {
            parse_progress_stdout(stdout, reporter, total_bytes, cancel_flag);
        });
        let stderr_handle = scope.spawn(|| -> String {
            let mut buf = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut buf);
            buf
        });

        // Poll for completion so we can kill the process on cancel.
        let status = loop {
            if cancel_flag.load(Ordering::SeqCst) {
                let _ = child.kill();
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    return Err(NativeError::Fatal(VfsError::IOError(format!(
                        "7z wait failed: {e}"
                    ))));
                }
            }
        };

        let _ = stdout_handle.join();
        let stderr_text = stderr_handle.join().unwrap_or_default();

        if cancel_flag.load(Ordering::SeqCst) {
            return Err(NativeError::Fatal(VfsError::IOError(
                "Cancelled".to_string(),
            )));
        }

        if !status.success() {
            let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string());
            let stderr_excerpt = stderr_text.trim();
            return Err(NativeError::Fatal(VfsError::IOError(format!(
                "7z exited with {code}: {}",
                if stderr_excerpt.is_empty() {
                    "no stderr"
                } else {
                    stderr_excerpt
                }
            ))));
        }

        // Land progress at 100% so the modal settles cleanly.
        reporter.set_total_bytes_copied(total_bytes);
        Ok(())
    });

    let _ = std::fs::remove_file(&listfile_path);
    result
}

fn derive_dest_root(targets: &[(String, PathBuf)]) -> Option<PathBuf> {
    let (first_inner, first_dest) = targets.first()?;
    let depth = inner_depth(first_inner);
    if depth == 0 {
        return None;
    }
    let mut candidate = first_dest.clone();
    for _ in 0..depth {
        if !candidate.pop() {
            return None;
        }
    }

    // Verify every target lands at <candidate>/<inner> (with platform
    // separators). If anyone disagrees, our caller stripped a prefix off the
    // inner paths and we can't reproduce that with `7z x -o<dir>`.
    for (inner, dest) in targets {
        let expected = candidate.join(inner_to_native(inner));
        if !paths_equal(&expected, dest) {
            return None;
        }
    }
    Some(candidate)
}

fn inner_depth(inner: &str) -> usize {
    inner
        .replace('\\', "/")
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .count()
}

fn inner_to_native(inner: &str) -> String {
    inner.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    // Component-wise comparison avoids tripping over trailing slashes /
    // mixed separators on Windows.
    a.components().eq(b.components())
}

fn write_listfile(path: &Path, targets: &[(String, PathBuf)]) -> Result<(), VfsError> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)
        .map_err(|e| VfsError::IOError(format!("cannot create 7z listfile: {e}")))?;
    for (inner, _) in targets {
        // 7-Zip's listfile parser accepts UTF-8 lines and matches against the
        // archive entry names verbatim. The names in our targets came out of
        // the 7z central index, so they round-trip cleanly.
        let normalized = inner.replace('\\', "/");
        writeln!(f, "{normalized}")
            .map_err(|e| VfsError::IOError(format!("cannot write 7z listfile: {e}")))?;
    }
    Ok(())
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}

/// Read 7z.exe's `-bsp1` stdout. Each progress snapshot is terminated by `\r`
/// (in-line update) with a final `\n`, so we split on either. A typical line
/// looks like ` 31% 28 - some/file/name` — percent, files-processed, optional
/// current entry. We only need the percent to drive overall progress.
fn parse_progress_stdout<R: Read>(
    reader: R,
    reporter: &ProgressReporter,
    total_bytes: u64,
    cancel_flag: &AtomicBool,
) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut buf = [0u8; 256];
    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        if !line.is_empty() {
                            apply_line(&line, reporter, total_bytes);
                            line.clear();
                        }
                    } else if b.is_ascii() {
                        line.push(b as char);
                    } else {
                        // Non-ASCII path bytes — keep them but don't push as
                        // raw `char`. We don't need a faithful render, just
                        // *something* for the status text.
                        line.push('?');
                    }
                }
            }
            Err(_) => break,
        }
    }
    if !line.is_empty() {
        apply_line(&line, reporter, total_bytes);
    }
}

fn apply_line(line: &str, reporter: &ProgressReporter, total_bytes: u64) {
    let trimmed = line.trim();
    let Some(pct_pos) = trimmed.find('%') else {
        return;
    };
    let pct_str = trimmed[..pct_pos].trim();
    let Ok(pct) = pct_str.parse::<u8>() else {
        return;
    };
    let pct = pct.min(100);
    let bytes = if total_bytes == 0 {
        0
    } else {
        ((total_bytes as u128) * (pct as u128) / 100) as u64
    };
    reporter.set_total_bytes_copied(bytes);

    // After "%" comes "<count> - <current file>". Pull the file name out so
    // the modal has something to display.
    let after_pct = trimmed[pct_pos + 1..].trim();
    let current_file = after_pct
        .split_once(" - ")
        .map(|(_, name)| name.trim().to_string())
        .filter(|s| !s.is_empty());

    let status = current_file
        .as_ref()
        .map(|f| format!("Extracting {f}"))
        .unwrap_or_else(|| format!("Extracting ({pct}%)"));

    reporter.send_snapshot(current_file, Some(status), total_bytes, bytes, false);
}
