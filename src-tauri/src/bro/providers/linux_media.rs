use crate::bro::model::{
    BroOption, BroPre, BroProtocol, BroSource, LaunchArg, LaunchRecipe, ResolvedBro,
};
use crate::bro::probe::{find_binary_on_path, flatpak_app_installed};
use crate::bro::providers::{mpv_playlist_args, BroDiscoveryContext, BroProvider};
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
                mpv_playlist_args(),
            ));
        }

        if let Some(program) = find_binary_on_path(&["vlc"]) {
            bros.push(native_command_bro(
                "native:vlc",
                "VLC",
                program.clone(),
                native_version(&program),
                vec![LaunchArg::TargetPlaylistFile {
                    flag: "".to_string(),
                }],
            ));
        }

        if flatpak_app_installed("io.mpv.Mpv") {
            bros.push(flatpak_mpv_bro());
        }

        if flatpak_app_installed("org.videolan.VLC") {
            bros.push(flatpak_media_bro("org.videolan.VLC", "VLC"));
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
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        program,
        args,
    )
}

fn flatpak_mpv_bro() -> ResolvedBro {
    flatpak_media_bro_with_args("io.mpv.Mpv", "mpv", mpv_playlist_args())
}

fn flatpak_media_bro(app_id: &str, name: &str) -> ResolvedBro {
    flatpak_media_bro_with_args(
        app_id,
        name,
        vec![LaunchArg::TargetPlaylistFile {
            flag: "--playlist=".to_string(),
        }],
    )
}

fn flatpak_media_bro_with_args(
    app_id: &str,
    name: &str,
    player_args: Vec<LaunchArg>,
) -> ResolvedBro {
    let mut args = vec![
        LaunchArg::Literal("run".to_string()),
        LaunchArg::Literal(app_id.to_string()),
    ];
    args.extend(player_args);
    resolved_command_bro(
        BroOption {
            id: format!("flatpak:{app_id}"),
            name: name.to_string(),
            version: flatpak_version(app_id),
            source: BroSource::Flatpak,
            detail: Some(app_id.to_string()),
            protocol: BroProtocol::Fnf,
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        "flatpak".to_string(),
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
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
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
    }
}
