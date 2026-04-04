use std::process::Command;

pub(crate) fn native_version(program: &str) -> String {
    Command::new(program)
        .arg("--version")
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
        .unwrap_or_else(|| "unknown".to_string())
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
