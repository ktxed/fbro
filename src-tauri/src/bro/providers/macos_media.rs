use crate::bro::model::{
    BroOption, BroPre, BroProtocol, BroSource, LaunchArg, LaunchRecipe, ResolvedBro,
};
use crate::bro::probe::{
    find_binary_on_path, macos_app_executable, macos_app_exists, macos_common_cli_candidates,
};
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::native_version;

pub struct MacOSMediaProvider;

impl BroProvider for MacOSMediaProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let _ = ctx.inputs;
        let mut bros = Vec::new();
        let mut seen_programs = std::collections::HashSet::new();

        if let Some(program) = find_mpv_program() {
            let version = native_version(&program);
            seen_programs.insert(program.clone());
            bros.push(native_command_bro(
                "native:mpv",
                "mpv",
                program,
                Some("mpv".to_string()),
                version,
                vec![LaunchArg::TargetPlaylistFile {
                    flag: "--playlist=".to_string(),
                }],
            ));
        }

        for bundle_path in macos_mpv_bundle_candidates() {
            if !macos_app_exists(&bundle_path) {
                continue;
            }
            let Some(program) = macos_app_executable(&bundle_path, "mpv") else {
                continue;
            };
            if !seen_programs.insert(program.clone()) {
                continue;
            }
            let version = native_version(&program);
            bros.push(native_command_bro(
                &format!("native:mpv-bundle:{bundle_path}"),
                "mpv",
                program,
                Some(bundle_path),
                version,
                vec![LaunchArg::TargetPlaylistFile {
                    flag: "--playlist=".to_string(),
                }],
            ));
        }

        let mut seen_vlc: std::collections::HashSet<String> = std::collections::HashSet::new();

        for bundle_path in macos_vlc_bundle_candidates() {
            if !macos_app_exists(&bundle_path) {
                continue;
            }
            let Some(program) = macos_app_executable(&bundle_path, "VLC") else {
                continue;
            };
            if !seen_vlc.insert(program.clone()) {
                continue;
            }
            let version = native_version(&program);
            bros.push(native_command_bro(
                &format!("native:vlc-bundle:{bundle_path}"),
                "VLC",
                program,
                Some(bundle_path),
                version,
                vec![LaunchArg::TargetPlaylistFile {
                    flag: "".to_string(),
                }],
            ));
        }

        bros
    }
}

fn find_mpv_program() -> Option<String> {
    find_binary_on_path(&["mpv"]).or_else(|| {
        macos_common_cli_candidates(&["mpv"])
            .into_iter()
            .find(|p| std::path::Path::new(p).is_file())
    })
}

fn native_command_bro(
    id: &str,
    name: &str,
    program: String,
    detail: Option<String>,
    version: String,
    args: Vec<LaunchArg>,
) -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: id.to_string(),
            name: name.to_string(),
            version,
            source: BroSource::Native,
            detail,
            protocol: BroProtocol::Fnf,
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        launch: LaunchRecipe::Command {
            program,
            cwd: None,
            args,
        },
    }
}

fn macos_mpv_bundle_candidates() -> Vec<String> {
    let mut candidates = vec!["/Applications/mpv.app".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            candidates.push(format!("{home}/Applications/mpv.app"));
        }
    }
    candidates
}

fn macos_vlc_bundle_candidates() -> Vec<String> {
    let mut candidates = vec!["/Applications/VLC.app".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            candidates.push(format!("{home}/Applications/VLC.app"));
        }
    }
    candidates
}

#[cfg(test)]
mod tests;
