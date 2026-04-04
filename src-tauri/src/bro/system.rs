#[cfg(target_os = "linux")]
use std::env;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::{env, path::PathBuf};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn find_executable(candidates: &[&str]) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        candidates.iter().find_map(|candidate| {
            let joined = dir.join(candidate);
            if joined.is_file() {
                Some(joined.to_string_lossy().to_string())
            } else {
                None
            }
        })
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn native_version(program: &str) -> String {
    command_version(program, "--version").unwrap_or_else(|| "unknown".to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn command_version(program: &str, version_arg: &str) -> Option<String> {
    Command::new(program)
        .arg(version_arg)
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.lines().find_map(extract_version).or_else(|| {
                stdout
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
            })
        })
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_app_bundle_executable(
    bundle_path: &str,
    executable_name: &str,
) -> Option<String> {
    let path = PathBuf::from(bundle_path);
    if !path.is_dir() {
        return None;
    }
    let executable = path.join("Contents").join("MacOS").join(executable_name);
    if executable.is_file() {
        Some(executable.to_string_lossy().to_string())
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn flatpak_installed(app_id: &str) -> bool {
    Command::new("flatpak")
        .arg("info")
        .arg("--show-ref")
        .arg(app_id)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub(crate) fn flatpak_version(app_id: &str) -> String {
    Command::new("flatpak")
        .arg("info")
        .arg(app_id)
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.lines().find_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix("Version:")
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(target_os = "linux")]
fn extract_version(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|token| {
        let trimmed =
            token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-');
        if trimmed.chars().any(|ch| ch.is_ascii_digit()) && trimmed.contains('.') {
            Some(trimmed.to_string())
        } else {
            None
        }
    })
}
