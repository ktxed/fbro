use std::env;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct BroInputItem {
    pub path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct BroOption {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: String,
    pub detail: Option<String>,
    pub protocol: String,
}

#[derive(Clone, Debug)]
enum BroLaunch {
    Command { program: String, args: Vec<String> },
    Flatpak { app_id: String },
}

#[derive(Clone, Debug)]
pub struct ResolvedBro {
    pub option: BroOption,
    launch: BroLaunch,
}

pub fn list_bros(_inputs: &[BroInputItem]) -> Vec<ResolvedBro> {
    #[cfg(target_os = "linux")]
    {
        linux_bros()
    }

    #[cfg(not(target_os = "linux"))]
    {
        vec![]
    }
}

pub fn launch_bro(inputs: &[BroInputItem], bro: &ResolvedBro) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("No input entries were resolved for this bro".to_string());
    }

    let (program, args) = build_launch_command(inputs, bro);
    let mut cmd = Command::new(&program);
    cmd.args(args);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn build_launch_command(inputs: &[BroInputItem], bro: &ResolvedBro) -> (String, Vec<String>) {
    match &bro.launch {
        BroLaunch::Command { program, args } => {
            let mut all_args = args.clone();
            all_args.extend(inputs.iter().map(|input| input.path.clone()));
            (program.clone(), all_args)
        }
        BroLaunch::Flatpak { app_id } => {
            let mut args = vec![
                "run".to_string(),
                "--file-forwarding".to_string(),
                app_id.clone(),
                "@@".to_string(),
            ];
            args.extend(inputs.iter().map(|input| input.path.clone()));
            args.push("@@".to_string());
            ("flatpak".to_string(), args)
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_bros() -> Vec<ResolvedBro> {
    let mut bros = Vec::new();

    if let Some(program) = find_executable(&["mpv"]) {
        bros.push(ResolvedBro {
            option: BroOption {
                id: "native:mpv".to_string(),
                name: "mpv".to_string(),
                version: native_version(&program),
                source: "native".to_string(),
                detail: Some(program.clone()),
                protocol: "fnf".to_string(),
            },
            launch: BroLaunch::Command {
                program,
                args: vec!["--".to_string()],
            },
        });
    }

    if let Some(program) = find_executable(&["vlc"]) {
        bros.push(ResolvedBro {
            option: BroOption {
                id: "native:vlc".to_string(),
                name: "VLC".to_string(),
                version: native_version(&program),
                source: "native".to_string(),
                detail: Some(program.clone()),
                protocol: "fnf".to_string(),
            },
            launch: BroLaunch::Command {
                program,
                args: vec![],
            },
        });
    }

    if flatpak_installed("io.mpv.Mpv") {
        bros.push(ResolvedBro {
            option: BroOption {
                id: "flatpak:io.mpv.Mpv".to_string(),
                name: "mpv".to_string(),
                version: flatpak_version("io.mpv.Mpv"),
                source: "flatpak".to_string(),
                detail: Some("io.mpv.Mpv".to_string()),
                protocol: "fnf".to_string(),
            },
            launch: BroLaunch::Flatpak {
                app_id: "io.mpv.Mpv".to_string(),
            },
        });
    }

    if flatpak_installed("org.videolan.VLC") {
        bros.push(ResolvedBro {
            option: BroOption {
                id: "flatpak:org.videolan.VLC".to_string(),
                name: "VLC".to_string(),
                version: flatpak_version("org.videolan.VLC"),
                source: "flatpak".to_string(),
                detail: Some("org.videolan.VLC".to_string()),
                protocol: "fnf".to_string(),
            },
            launch: BroLaunch::Flatpak {
                app_id: "org.videolan.VLC".to_string(),
            },
        });
    }

    bros
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn native_version(program: &str) -> String {
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
fn flatpak_installed(app_id: &str) -> bool {
    Command::new("flatpak")
        .arg("info")
        .arg("--show-ref")
        .arg(app_id)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn flatpak_version(app_id: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{build_launch_command, BroInputItem, BroLaunch, BroOption, ResolvedBro};

    fn sample_inputs() -> Vec<BroInputItem> {
        vec![
            BroInputItem {
                path: "/tmp/a.mp4".to_string(),
                name: "a.mp4".to_string(),
                kind: "file".to_string(),
            },
            BroInputItem {
                path: "/tmp/b.mp3".to_string(),
                name: "b.mp3".to_string(),
                kind: "file".to_string(),
            },
        ]
    }

    fn sample_bro(launch: BroLaunch) -> ResolvedBro {
        ResolvedBro {
            option: BroOption {
                id: "bro".to_string(),
                name: "bro".to_string(),
                version: "unknown".to_string(),
                source: "test".to_string(),
                detail: None,
                protocol: "fnf".to_string(),
            },
            launch,
        }
    }

    #[test]
    fn builds_native_mpv_command() {
        let bro = sample_bro(BroLaunch::Command {
            program: "mpv".to_string(),
            args: vec!["--".to_string()],
        });
        let (program, args) = build_launch_command(&sample_inputs(), &bro);
        assert_eq!(program, "mpv");
        assert_eq!(args, vec!["--", "/tmp/a.mp4", "/tmp/b.mp3"]);
    }

    #[test]
    fn builds_flatpak_command() {
        let bro = sample_bro(BroLaunch::Flatpak {
            app_id: "org.videolan.VLC".to_string(),
        });
        let (program, args) = build_launch_command(&sample_inputs(), &bro);
        assert_eq!(program, "flatpak");
        assert_eq!(
            args,
            vec![
                "run",
                "--file-forwarding",
                "org.videolan.VLC",
                "@@",
                "/tmp/a.mp4",
                "/tmp/b.mp3",
                "@@",
            ]
        );
    }
}
