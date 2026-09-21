pub mod thumbnail;
pub mod udisks2;
pub mod volume_watcher;

use super::{
    append_supplemental_apps, ExternalClipboardOp, ExternalClipboardPayload, OpenWithAppOption,
    OpenWithLaunch, Platform, ResolvedOpenWithApp, ShellCommandInstallResult,
    ShellCommandRemoveResult, ShellCommandStatus, VolumeInfo,
};
use gio::prelude::*;
use libc;
use std::collections::HashSet;
use std::env;
use std::path::Path;
use std::process::Command;

pub struct LinuxPlatform;

#[derive(Clone, Copy)]
enum LinuxLaunchMode {
    Files,
    Uris,
}

fn decode_proc_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let a = bytes[i + 1];
            let b = bytes[i + 2];
            let c = bytes[i + 3];
            if (b'0'..=b'7').contains(&a)
                && (b'0'..=b'7').contains(&b)
                && (b'0'..=b'7').contains(&c)
            {
                out.push((a - b'0') * 64 + (b - b'0') * 8 + (c - b'0'));
                i += 4;
                continue;
            }
        }

        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

impl Platform for LinuxPlatform {
    fn open_file(&self, path: &str) {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open the link: {error}"))
    }

    fn get_thumbnail(&self, path: &str, size: u32) -> Option<(Vec<u8>, &'static str)> {
        thumbnail::generate(path, size)
    }

    fn disk_space(&self, path: &str) -> Option<(u64, u64)> {
        use std::ffi::CString;
        let cpath = CString::new(path).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) };
        if ret != 0 {
            return None;
        }
        let block_size = stat.f_frsize as u64;
        let total = stat.f_blocks as u64 * block_size;
        let free = stat.f_bavail as u64 * block_size; // bavail = available to unprivileged user
        Some((free, total))
    }

    fn open_terminal(&self, path: &str) {
        if let Ok(term) = std::env::var("TERMINAL") {
            let _ = Command::new(&term).current_dir(path).spawn();
            return;
        }
        // (terminal, working-directory flag)
        let candidates: &[(&str, Option<&str>)] = &[
            ("cosmic-term", Some("--working-directory")),
            ("gnome-terminal", Some("--working-directory")),
            ("xfce4-terminal", Some("--working-directory")),
            ("mate-terminal", Some("--working-directory")),
            ("tilix", Some("--working-directory")),
            ("kitty", Some("--directory")),
            ("alacritty", Some("--working-directory")),
            ("xterm", None),
            ("rxvt", None),
        ];
        for (term, flag) in candidates {
            let mut cmd = Command::new(term);
            if let Some(f) = flag {
                cmd.arg(f).arg(path);
            }
            cmd.current_dir(path);
            if cmd.spawn().is_ok() {
                return;
            }
        }
    }

    fn reveal_in_file_manager(&self, path: &str, _kind: &str) {
        let target = Path::new(path).parent().unwrap_or_else(|| Path::new(path));
        let _ = Command::new("xdg-open").arg(target).spawn();
    }

    fn list_open_with_apps(
        &self,
        path: &str,
        kind: &str,
    ) -> Result<Vec<ResolvedOpenWithApp>, String> {
        let content_type = if kind == "directory" {
            "inode/directory".to_string()
        } else {
            linux_content_type_for_path(path)
        };

        let default_key = gio::AppInfo::default_for_type(&content_type, false)
            .as_ref()
            .and_then(linux_app_key);
        let mut apps = Vec::new();
        let mut seen = HashSet::new();

        let mut candidates = Vec::new();
        if let Some(default) = gio::AppInfo::default_for_type(&content_type, false) {
            candidates.push(default);
        }
        candidates.extend(gio::AppInfo::recommended_for_type(&content_type));
        candidates.extend(gio::AppInfo::all_for_type(&content_type));

        for app in candidates {
            let Some(app_key) = linux_app_key(&app) else {
                continue;
            };
            if !seen.insert(app_key.clone()) {
                continue;
            }
            let is_default = default_key.as_deref() == Some(app_key.as_str());
            let launch_mode = if app.supports_files() {
                LinuxLaunchMode::Files
            } else if app.supports_uris() {
                LinuxLaunchMode::Uris
            } else {
                continue;
            };

            let label = app.display_name().to_string().trim().to_string();
            let label = if label.is_empty() {
                app.name().to_string()
            } else {
                label
            };
            let detail = path_buf_to_string(app.executable());

            apps.push(ResolvedOpenWithApp {
                option: OpenWithAppOption {
                    id: format!("gio:{app_key}"),
                    label,
                    detail,
                    is_default,
                    source: "system".to_string(),
                },
                launch: OpenWithLaunch::LinuxApp {
                    content_type: content_type.clone(),
                    app_key: format!("{app_key}::{}", linux_launch_mode_key(launch_mode)),
                },
            });
        }

        append_supplemental_apps(&mut apps, linux_supplemental_apps());
        Ok(apps)
    }

    fn launch_open_with(&self, path: &str, app: &ResolvedOpenWithApp) -> Result<(), String> {
        match &app.launch {
            OpenWithLaunch::LinuxApp {
                content_type,
                app_key,
            } => {
                let (app_key, launch_mode) = parse_linux_app_key(app_key);
                let file = gio::File::for_path(path);
                let app_info = gio::AppInfo::default_for_type(content_type, false)
                    .into_iter()
                    .chain(gio::AppInfo::recommended_for_type(content_type))
                    .chain(gio::AppInfo::all_for_type(content_type))
                    .find(|candidate| linux_app_key(candidate).as_deref() == Some(app_key))
                    .ok_or_else(|| "The selected application is no longer available".to_string())?;

                match launch_mode {
                    LinuxLaunchMode::Files => app_info
                        .launch(&[file], None::<&gio::AppLaunchContext>)
                        .map_err(|e| e.to_string()),
                    LinuxLaunchMode::Uris => {
                        let uri = gio::File::for_path(path).uri();
                        app_info
                            .launch_uris(&[uri.as_str()], None::<&gio::AppLaunchContext>)
                            .map_err(|e| e.to_string())
                    }
                }
            }
            OpenWithLaunch::Default => {
                self.open_file(path);
                Ok(())
            }
            OpenWithLaunch::Command { program, args } => {
                let mut cmd = Command::new(program);
                cmd.args(args).arg(path);
                cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
            }
            OpenWithLaunch::MacApplication { .. } => {
                Err("The selected application is not available on Linux".to_string())
            }
            OpenWithLaunch::WindowsHandlerInvoke { .. } => {
                Err("The selected application is not available on Linux".to_string())
            }
        }
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        // Try known editors in preference order. Spawn by command name so the
        // OS PATH lookup at spawn time is used (more reliable than a pre-check).
        const PREFERRED: &[&str] = &["code", "codium", "cursor", "zed", "subl"];
        for &editor in PREFERRED {
            if Command::new(editor).arg(path).spawn().is_ok() {
                return Ok(path.to_string());
            }
        }
        // Fall back to xdg-open (system default handler).
        let output = Command::new("xdg-open")
            .arg(path)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(path.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(stderr.trim().to_string())
        }
    }

    fn find_icon(&self, icon_name: &str) -> Option<(Vec<u8>, &'static str)> {
        const THEMES: &[&str] = &["Pop", "Adwaita", "hicolor"];
        const SIZES: &[&str] = &["48x48", "scalable", "32x32", "24x24"];
        const CATS: &[&str] = &["mimetypes", "places", "emblems"];

        for theme in THEMES {
            for size in SIZES {
                for cat in CATS {
                    let svg = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.svg");
                    if let Ok(data) = std::fs::read(&svg) {
                        return Some((data, "image/svg+xml"));
                    }
                    let png = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.png");
                    if let Ok(data) = std::fs::read(&png) {
                        return Some((data, "image/png"));
                    }
                }
            }
        }
        None
    }

    fn list_volumes(&self) -> Vec<VolumeInfo> {
        const FALLBACK: &str = "/";
        const WHITELIST: &[&str] = &[
            "ext2", "ext3", "ext4", "btrfs", "xfs", "zfs", "vfat", "exfat", "ntfs", "ntfs3",
            "f2fs", "jfs", "hfsplus", "fuseblk", "nfs", "nfs4", "cifs",
        ];

        let contents = match std::fs::read_to_string("/proc/mounts") {
            Ok(c) => c,
            Err(_) => {
                return vec![VolumeInfo {
                    id: "vol-root".to_string(),
                    name: "/".to_string(),
                    root_path: FALLBACK.to_string(),
                    mounted: true,
                    device_path: None,
                }];
            }
        };

        let mut seen = std::collections::HashSet::new();
        let mut volumes: Vec<VolumeInfo> = Vec::new();

        for line in contents.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 {
                continue;
            }
            let mountpoint = decode_proc_mount_field(fields[1]);
            let fstype = fields[2];

            if !WHITELIST.contains(&fstype) {
                continue;
            }
            if seen.contains(&mountpoint) {
                continue;
            }
            seen.insert(mountpoint.clone());

            let (id, name) = if mountpoint == "/" {
                ("vol-root".to_string(), "/".to_string())
            } else {
                let id = format!("vol{}", mountpoint.replace('/', "-"));
                let name = mountpoint
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&mountpoint)
                    .to_string();
                (id, name)
            };

            volumes.push(VolumeInfo {
                id,
                name,
                root_path: mountpoint,
                mounted: true,
                device_path: None,
            });
        }

        if volumes.is_empty() {
            return vec![VolumeInfo {
                id: "vol-root".to_string(),
                name: "/".to_string(),
                root_path: FALLBACK.to_string(),
                mounted: true,
                device_path: None,
            }];
        }

        // Sort: "/" first, then alphabetically by root_path
        volumes.sort_by(|a, b| {
            if a.root_path == "/" {
                std::cmp::Ordering::Less
            } else if b.root_path == "/" {
                std::cmp::Ordering::Greater
            } else {
                a.root_path.cmp(&b.root_path)
            }
        });

        volumes
    }

    fn read_external_file_clipboard(&self) -> Result<Option<ExternalClipboardPayload>, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        gtk::glib::MainContext::default().invoke(move || {
            let result = read_linux_external_clipboard();
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| {
            "Failed to receive clipboard data from the GTK main context.".to_string()
        })?
    }

    fn write_external_file_clipboard(
        &self,
        payload: &ExternalClipboardPayload,
    ) -> Result<(), String> {
        let payload = payload.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        gtk::glib::MainContext::default().invoke(move || {
            let result = write_linux_external_clipboard(&payload);
            let _ = tx.send(result);
        });
        rx.recv()
            .map_err(|_| "Failed to write clipboard data from the GTK main context.".to_string())?
    }

    fn write_text_clipboard(&self, text: &str) -> Result<(), String> {
        let text = text.to_string();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        gtk::glib::MainContext::default().invoke(move || {
            let result = write_linux_text_clipboard(&text);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| {
            "Failed to write text to clipboard from the GTK main context.".to_string()
        })?
    }

    fn home_dir(&self) -> String {
        // Prefer $HOME; fall back to /etc/passwd entry for current uid
        if let Ok(h) = std::env::var("HOME") {
            if !h.is_empty() {
                return h;
            }
        }

        // Fallback: parse /etc/passwd for current uid
        if let Ok(uid) = std::env::var("UID")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or(())
            .or_else(|_| -> Result<u32, ()> {
                // SAFETY: getuid() is always safe to call
                #[cfg(unix)]
                {
                    Ok(libc_uid())
                }
                #[cfg(not(unix))]
                Err(())
            })
        {
            if let Ok(contents) = std::fs::read_to_string("/etc/passwd") {
                for line in contents.lines() {
                    let fields: Vec<&str> = line.split(':').collect();
                    if fields.len() >= 6 {
                        if fields[2].parse::<u32>().ok() == Some(uid) {
                            return fields[5].to_string();
                        }
                    }
                }
            }
        }

        "/".to_string()
    }

    fn probe_fbro_command(&self) -> Result<ShellCommandStatus, String> {
        super::shell_command::probe_fbro_command()
    }

    fn install_fbro_command(&self, exe: &Path) -> Result<ShellCommandInstallResult, String> {
        super::shell_command::install_fbro_command(exe)
    }

    fn remove_fbro_command(&self) -> Result<ShellCommandRemoveResult, String> {
        super::shell_command::remove_fbro_command()
    }
}

#[cfg(test)]
mod tests {
    use super::decode_proc_mount_field;

    #[test]
    fn decodes_proc_mount_octal_escapes() {
        assert_eq!(
            decode_proc_mount_field("/run/media/me/Extreme\\040SSD"),
            "/run/media/me/Extreme SSD"
        );
        assert_eq!(
            decode_proc_mount_field("/mnt/a\\011b\\012c\\134d"),
            "/mnt/a\tb\nc\\d"
        );
    }

    #[test]
    fn leaves_non_octal_backslashes_alone() {
        assert_eq!(decode_proc_mount_field("/mnt/a\\x20b"), "/mnt/a\\x20b");
        assert_eq!(decode_proc_mount_field("/mnt/a\\999b"), "/mnt/a\\999b");
    }
}

fn linux_content_type_for_path(path: &str) -> String {
    let file = gio::File::for_path(path);
    if let Ok(info) = file.query_info(
        gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE,
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    ) {
        if let Some(content_type) = info.content_type() {
            let value = content_type.to_string();
            if !value.is_empty() {
                return value;
            }
        }
    }

    gio::content_type_guess(Path::new(path).file_name().map(Path::new), &[])
        .0
        .to_string()
}

fn linux_app_key(app: &gio::AppInfo) -> Option<String> {
    app.id()
        .map(|id| id.to_string())
        .or_else(|| path_buf_to_string(app.executable()))
        .filter(|value| !value.trim().is_empty())
}

fn linux_launch_mode_key(mode: LinuxLaunchMode) -> &'static str {
    match mode {
        LinuxLaunchMode::Files => "files",
        LinuxLaunchMode::Uris => "uris",
    }
}

fn parse_linux_app_key(value: &str) -> (&str, LinuxLaunchMode) {
    if let Some((app_key, mode)) = value.rsplit_once("::") {
        let launch_mode = if mode == "uris" {
            LinuxLaunchMode::Uris
        } else {
            LinuxLaunchMode::Files
        };
        (app_key, launch_mode)
    } else {
        (value, LinuxLaunchMode::Files)
    }
}

fn path_buf_to_string(path: std::path::PathBuf) -> Option<String> {
    let value = path.to_string_lossy().trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn read_linux_external_clipboard() -> Result<Option<ExternalClipboardPayload>, String> {
    if !gtk::is_initialized() {
        gtk::init().map_err(|error| error.to_string())?;
    }

    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);

    let gnome_atom = gdk::Atom::intern("x-special/gnome-copied-files");
    if let Some(selection) = clipboard.wait_for_contents(&gnome_atom) {
        if let Ok(text) = String::from_utf8(selection.data()) {
            if let Some(payload) = parse_gnome_clipboard_payload(&text) {
                return Ok(Some(payload));
            }
        }
    }

    let source_paths: Vec<String> = clipboard
        .wait_for_uris()
        .into_iter()
        .filter_map(|uri| gio::File::for_uri(uri.as_str()).path())
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    if source_paths.is_empty() {
        return Ok(None);
    }

    let kde_cut_atom = gdk::Atom::intern("application/x-kde-cutselection");
    let op = clipboard
        .wait_for_contents(&kde_cut_atom)
        .map(|selection| selection.data())
        .and_then(|bytes| bytes.first().copied())
        .map(|byte| {
            if byte == b'1' {
                ExternalClipboardOp::Move
            } else {
                ExternalClipboardOp::Copy
            }
        })
        .unwrap_or(ExternalClipboardOp::Copy);

    Ok(Some(ExternalClipboardPayload { op, source_paths }))
}

fn write_linux_external_clipboard(payload: &ExternalClipboardPayload) -> Result<(), String> {
    if !gtk::is_initialized() {
        gtk::init().map_err(|error| error.to_string())?;
    }

    const GNOME_INFO: u32 = 1;
    const URI_INFO: u32 = 2;
    const KDE_INFO: u32 = 3;

    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    let uri_strings: Vec<String> = payload
        .source_paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().to_string())
        .collect();

    if uri_strings.is_empty() {
        return Err("No files are available to export to the host clipboard.".to_string());
    }

    let op_line = match payload.op {
        ExternalClipboardOp::Copy => "copy",
        ExternalClipboardOp::Move => "cut",
    };
    let gnome_payload = format!("{op_line}\n{}\n", uri_strings.join("\n"));
    let kde_payload = [match payload.op {
        ExternalClipboardOp::Move => b'1',
        ExternalClipboardOp::Copy => b'0',
    }];
    let targets = vec![
        gtk::TargetEntry::new(
            "x-special/gnome-copied-files",
            gtk::TargetFlags::OTHER_APP,
            GNOME_INFO,
        ),
        gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP, URI_INFO),
        gtk::TargetEntry::new(
            "application/x-kde-cutselection",
            gtk::TargetFlags::OTHER_APP,
            KDE_INFO,
        ),
    ];

    let success =
        clipboard.set_with_data(
            &targets,
            move |_clipboard, selection_data, info| match info {
                GNOME_INFO => {
                    let atom = gdk::Atom::intern("x-special/gnome-copied-files");
                    selection_data.set(&atom, 8, gnome_payload.as_bytes());
                }
                URI_INFO => {
                    let uris = uri_strings.iter().map(String::as_str).collect::<Vec<_>>();
                    let _ = selection_data.set_uris(&uris);
                }
                KDE_INFO => {
                    let atom = gdk::Atom::intern("application/x-kde-cutselection");
                    selection_data.set(&atom, 8, &kde_payload);
                }
                _ => {}
            },
        );

    if !success {
        return Err("Failed to publish the host clipboard payload.".to_string());
    }

    clipboard.store();
    Ok(())
}

fn write_linux_text_clipboard(text: &str) -> Result<(), String> {
    if !gtk::is_initialized() {
        gtk::init().map_err(|e| e.to_string())?;
    }
    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    clipboard.set_text(text);
    clipboard.store();
    Ok(())
}

fn parse_gnome_clipboard_payload(text: &str) -> Option<ExternalClipboardPayload> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let op = match lines.next()? {
        "copy" => ExternalClipboardOp::Copy,
        "cut" => ExternalClipboardOp::Move,
        _ => return None,
    };

    let source_paths = lines
        .filter_map(|uri| gio::File::for_uri(uri).path())
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    if source_paths.is_empty() {
        None
    } else {
        Some(ExternalClipboardPayload { op, source_paths })
    }
}

fn linux_supplemental_apps() -> Vec<ResolvedOpenWithApp> {
    supplemental_command_apps(&[
        ("supplement:code", "Visual Studio Code", &["code"]),
        ("supplement:codium", "VSCodium", &["codium"]),
        ("supplement:cursor", "Cursor", &["cursor"]),
        ("supplement:zed", "Zed", &["zed"]),
        ("supplement:sublime", "Sublime Text", &["subl"]),
        ("supplement:idea", "IntelliJ IDEA", &["idea"]),
        ("supplement:pycharm", "PyCharm", &["pycharm"]),
        ("supplement:webstorm", "WebStorm", &["webstorm"]),
        ("supplement:clion", "CLion", &["clion"]),
        ("supplement:goland", "GoLand", &["goland"]),
        ("supplement:rustrover", "RustRover", &["rustrover"]),
    ])
}

fn supplemental_command_apps(specs: &[(&str, &str, &[&str])]) -> Vec<ResolvedOpenWithApp> {
    specs
        .iter()
        .filter_map(|(id, label, executables)| {
            find_executable(executables).map(|program| ResolvedOpenWithApp {
                option: OpenWithAppOption {
                    id: (*id).to_string(),
                    label: (*label).to_string(),
                    detail: Some(program.clone()),
                    is_default: false,
                    source: "supplement".to_string(),
                },
                launch: OpenWithLaunch::Command {
                    program,
                    args: vec![],
                },
            })
        })
        .collect()
}

fn find_executable(candidates: &[&str]) -> Option<String> {
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

#[cfg(unix)]
fn libc_uid() -> u32 {
    // Use std::os::unix instead of pulling in libc crate
    // nix syscall via inline assembly is overkill; use /proc/self/status
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(uid_str) = rest.split_whitespace().next() {
                    if let Ok(uid) = uid_str.parse::<u32>() {
                        return uid;
                    }
                }
            }
        }
    }
    u32::MAX
}
