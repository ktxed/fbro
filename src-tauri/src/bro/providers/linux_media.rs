use crate::bro::model::{BroOption, BroProtocol, BroSource, LaunchRecipe, ResolvedBro};
use crate::bro::probe::{find_binary_on_path, flatpak_app_installed};
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::{flatpak_version, native_version};

pub struct LinuxMediaProvider;

impl BroProvider for LinuxMediaProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let _ = ctx.inputs;
        let mut bros = Vec::new();

        if let Some(program) = find_binary_on_path(&["mpv"]) {
            bros.push(native_command_bro(
                "native:mpv",
                "mpv",
                program.clone(),
                native_version(&program),
                vec!["--".to_string()],
            ));
        }

        if let Some(program) = find_binary_on_path(&["vlc"]) {
            bros.push(native_command_bro(
                "native:vlc",
                "VLC",
                program.clone(),
                native_version(&program),
                vec![],
            ));
        }

        if flatpak_app_installed("io.mpv.Mpv") {
            bros.push(flatpak_command_bro("io.mpv.Mpv", "mpv"));
        }

        if flatpak_app_installed("org.videolan.VLC") {
            bros.push(flatpak_command_bro("org.videolan.VLC", "VLC"));
        }

        bros
    }
}

fn native_command_bro(
    id: &str,
    name: &str,
    program: String,
    version: String,
    args_before_inputs: Vec<String>,
) -> ResolvedBro {
    resolved_command_bro(
        BroOption {
            id: id.to_string(),
            name: name.to_string(),
            version,
            source: BroSource::Native,
            detail: Some(program.clone()),
            protocol: BroProtocol::Fnf,
        },
        program,
        args_before_inputs,
        vec![],
    )
}

fn flatpak_command_bro(app_id: &str, name: &str) -> ResolvedBro {
    resolved_command_bro(
        BroOption {
            id: format!("flatpak:{app_id}"),
            name: name.to_string(),
            version: flatpak_version(app_id),
            source: BroSource::Flatpak,
            detail: Some(app_id.to_string()),
            protocol: BroProtocol::Fnf,
        },
        "flatpak".to_string(),
        vec![
            "run".to_string(),
            "--file-forwarding".to_string(),
            app_id.to_string(),
            "@@".to_string(),
        ],
        vec!["@@".to_string()],
    )
}

fn resolved_command_bro(
    option: BroOption,
    program: String,
    args_before_inputs: Vec<String>,
    args_after_inputs: Vec<String>,
) -> ResolvedBro {
    ResolvedBro {
        option,
        launch: LaunchRecipe::Command {
            program,
            args_before_inputs,
            args_after_inputs,
        },
    }
}
