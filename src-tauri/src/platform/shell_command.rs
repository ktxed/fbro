use std::path::{Path, PathBuf};

use super::{ShellCommandInstallResult, ShellCommandRemoveResult, ShellCommandStatus};

const COMMAND_NAME: &str = "fbro";
const MANAGED_MARKER: &str = "# Managed by fbro.";

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn fbro_command_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set.".to_string())?;
    Ok(Path::new(&home)
        .join(".local")
        .join("bin")
        .join(COMMAND_NAME))
}

fn command_is_managed(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|contents| {
            has_managed_marker(&contents) || looks_like_legacy_managed_script(&contents)
        })
        .unwrap_or(false)
}

fn has_managed_marker(contents: &str) -> bool {
    contents.lines().any(|line| line.trim() == MANAGED_MARKER)
}

fn looks_like_legacy_managed_script(contents: &str) -> bool {
    contents.starts_with("#!/bin/sh\n")
        && contents.contains("\nFBRO_BIN=")
        && contents.contains("\"$FBRO_BIN\" --fbro-open \"$1\" >/dev/null 2>&1 &")
        && contents.contains("\"$FBRO_BIN\" >/dev/null 2>&1 &")
        && contents.trim_end().ends_with("exit 0")
}

fn find_executable_on_path(command: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(command);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

pub fn probe_fbro_command() -> Result<ShellCommandStatus, String> {
    let managed_path = fbro_command_path()?;
    if managed_path.is_file() {
        return Ok(ShellCommandStatus {
            command: COMMAND_NAME.to_string(),
            path: Some(managed_path.to_string_lossy().to_string()),
            installed: true,
            managed: command_is_managed(&managed_path),
        });
    }

    if let Some(path) = find_executable_on_path(COMMAND_NAME) {
        return Ok(ShellCommandStatus {
            command: COMMAND_NAME.to_string(),
            path: Some(path.to_string_lossy().to_string()),
            installed: true,
            managed: false,
        });
    }

    Ok(ShellCommandStatus {
        command: COMMAND_NAME.to_string(),
        path: None,
        installed: false,
        managed: false,
    })
}

pub fn install_fbro_command(exe: &Path) -> Result<ShellCommandInstallResult, String> {
    use std::os::unix::fs::PermissionsExt;

    let script_path = fbro_command_path()?;
    if script_path.exists() && !command_is_managed(&script_path) {
        return Err(format!(
            "{} already exists and is not managed by fbro.",
            script_path.display()
        ));
    }

    let bin_dir = script_path
        .parent()
        .ok_or_else(|| "Cannot locate fbro command directory.".to_string())?;
    std::fs::create_dir_all(bin_dir)
        .map_err(|e| format!("Cannot create {}: {e}", bin_dir.display()))?;

    let exe_quoted = shell_single_quote(&exe.to_string_lossy());
    let script = format!(
        "#!/bin/sh\n\
         {MANAGED_MARKER}\n\
         FBRO_BIN={exe_quoted}\n\
         if [ \"$#\" -gt 0 ]; then\n\
         \t\"$FBRO_BIN\" --fbro-open \"$1\" >/dev/null 2>&1 &\n\
         else\n\
         \t\"$FBRO_BIN\" >/dev/null 2>&1 &\n\
         fi\n\
         exit 0\n"
    );
    std::fs::write(&script_path, script)
        .map_err(|e| format!("Cannot write {}: {e}", script_path.display()))?;
    let mut permissions = std::fs::metadata(&script_path)
        .map_err(|e| format!("Cannot stat {}: {e}", script_path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions)
        .map_err(|e| format!("Cannot chmod {}: {e}", script_path.display()))?;

    Ok(ShellCommandInstallResult {
        path: script_path.to_string_lossy().to_string(),
        command: COMMAND_NAME.to_string(),
    })
}

pub fn remove_fbro_command() -> Result<ShellCommandRemoveResult, String> {
    let script_path = fbro_command_path()?;
    if !script_path.exists() {
        return Err(format!(
            "The managed fbro command is not installed at {}.",
            script_path.display()
        ));
    }
    if !command_is_managed(&script_path) {
        return Err(format!(
            "{} exists but is not managed by fbro.",
            script_path.display()
        ));
    }

    std::fs::remove_file(&script_path)
        .map_err(|e| format!("Cannot remove {}: {e}", script_path.display()))?;
    Ok(ShellCommandRemoveResult {
        path: script_path.to_string_lossy().to_string(),
        command: COMMAND_NAME.to_string(),
    })
}
