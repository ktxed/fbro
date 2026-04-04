use crate::bro::model::{BroOption, BroProtocol, BroSource, LaunchArg, LaunchRecipe, ResolvedBro};
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
                vec![LaunchArg::Literal("--".to_string()), LaunchArg::TargetPaths],
            ));
        }

        if let Some(program) = find_binary_on_path(&["vlc"]) {
            bros.push(native_command_bro(
                "native:vlc",
                "VLC",
                program.clone(),
                native_version(&program),
                vec![LaunchArg::TargetPaths],
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
    args: Vec<LaunchArg>,
) -> ResolvedBro {
    resolved_command_bro(
        BroOption {
            id: id.to_string(),
            name: name.to_string(),
            version,
            source: BroSource::Native,
            detail: Some(program.clone()),
            protocol: BroProtocol::Fnf,
            inputs: vec![],
        },
        program,
        args,
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
            inputs: vec![],
        },
        "flatpak".to_string(),
        vec![
            LaunchArg::Literal("run".to_string()),
            LaunchArg::Literal("--file-forwarding".to_string()),
            LaunchArg::Literal(app_id.to_string()),
            LaunchArg::Literal("@@".to_string()),
            LaunchArg::TargetPaths,
            LaunchArg::Literal("@@".to_string()),
        ],
    )
}

fn resolved_command_bro(option: BroOption, program: String, args: Vec<LaunchArg>) -> ResolvedBro {
    ResolvedBro {
        option,
        launch: LaunchRecipe::Command {
            program,
            cwd: None,
            args,
        },
        output_path_input_id: None,
    }
}
