use crate::bro::model::{BroOption, BroProtocol, BroSource, LaunchRecipe, ResolvedBro};
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::system::{command_version, macos_app_bundle_executable, native_version};

pub struct MacOSMediaProvider;

impl BroProvider for MacOSMediaProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let _ = ctx.inputs;
        let mut bros = Vec::new();
        let mut seen_programs = std::collections::HashSet::new();

        if let Some(version) = command_version("mpv", "-version") {
            seen_programs.insert("mpv".to_string());
            bros.push(native_command_bro(
                "native:mpv",
                "mpv",
                "mpv".to_string(),
                Some("mpv".to_string()),
                version,
                vec!["--".to_string()],
            ));
        }

        for bundle_path in macos_mpv_bundle_candidates() {
            let Some(program) = macos_app_bundle_executable(&bundle_path, "mpv") else {
                continue;
            };
            if !seen_programs.insert(program.clone()) {
                continue;
            }
            bros.push(native_command_bro(
                &format!("native:mpv-bundle:{bundle_path}"),
                "mpv",
                program,
                Some(bundle_path),
                native_version(&program),
                vec!["--".to_string()],
            ));
        }

        bros
    }
}

fn native_command_bro(
    id: &str,
    name: &str,
    program: String,
    detail: Option<String>,
    version: String,
    args_before_inputs: Vec<String>,
) -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: id.to_string(),
            name: name.to_string(),
            version,
            source: BroSource::Native,
            detail,
            protocol: BroProtocol::Fnf,
        },
        launch: LaunchRecipe::Command {
            program,
            args_before_inputs,
            args_after_inputs: vec![],
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

#[cfg(test)]
mod tests {
    use super::macos_mpv_bundle_candidates;

    #[test]
    fn includes_global_applications_bundle_path() {
        let candidates = macos_mpv_bundle_candidates();
        assert!(candidates
            .iter()
            .any(|path| path == "/Applications/mpv.app"));
    }
}
