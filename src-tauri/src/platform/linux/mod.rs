pub mod udisks2;
pub mod volume_watcher;

use super::{
    append_supplemental_apps, OpenWithAppOption, OpenWithLaunch, Platform, ResolvedOpenWithApp,
    VolumeInfo,
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

impl Platform for LinuxPlatform {
    fn open_file(&self, path: &str) {
        let _ = Command::new("xdg-open").arg(path).spawn();
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
        }
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
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
            let mountpoint = fields[1];
            let fstype = fields[2];

            if !WHITELIST.contains(&fstype) {
                continue;
            }
            if seen.contains(mountpoint) {
                continue;
            }
            seen.insert(mountpoint.to_string());

            let (id, name) = if mountpoint == "/" {
                ("vol-root".to_string(), "/".to_string())
            } else {
                let id = format!("vol{}", mountpoint.replace('/', "-"));
                let name = mountpoint
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(mountpoint)
                    .to_string();
                (id, name)
            };

            volumes.push(VolumeInfo {
                id,
                name,
                root_path: mountpoint.to_string(),
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
