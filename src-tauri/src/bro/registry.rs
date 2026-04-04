use std::process::Command;

use crate::bro::model::{BroInputItem, ResolvedBro};
use crate::bro::providers::{builtin_providers, BroDiscoveryContext};

pub fn list_bros(inputs: &[BroInputItem]) -> Vec<ResolvedBro> {
    let ctx = BroDiscoveryContext { inputs };
    builtin_providers()
        .into_iter()
        .flat_map(|provider| provider.list(&ctx))
        .collect()
}

pub fn launch_bro(inputs: &[BroInputItem], bro: &ResolvedBro) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("No input entries were resolved for this bro".to_string());
    }

    let (program, args) = bro.build_command(inputs);
    let mut cmd = Command::new(&program);
    cmd.args(args);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{launch_bro, list_bros};
    use crate::bro::model::LaunchRecipe;
    use crate::bro::{BroInputItem, BroOption, BroProtocol, BroSource, ResolvedBro};

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

    fn sample_bro(source: BroSource, launch: LaunchRecipe) -> ResolvedBro {
        ResolvedBro {
            option: BroOption {
                id: "bro".to_string(),
                name: "bro".to_string(),
                version: "unknown".to_string(),
                source,
                detail: None,
                protocol: BroProtocol::Fnf,
            },
            launch,
        }
    }

    #[test]
    fn builds_native_mpv_command() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "mpv".to_string(),
                args_before_inputs: vec!["--".to_string()],
                args_after_inputs: vec![],
            },
        );
        let (program, args) = bro.build_command(&sample_inputs());
        assert_eq!(program, "mpv");
        assert_eq!(args, vec!["--", "/tmp/a.mp4", "/tmp/b.mp3"]);
    }

    #[test]
    fn builds_native_vlc_command() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "vlc".to_string(),
                args_before_inputs: vec![],
                args_after_inputs: vec![],
            },
        );
        let (program, args) = bro.build_command(&sample_inputs());
        assert_eq!(program, "vlc");
        assert_eq!(args, vec!["/tmp/a.mp4", "/tmp/b.mp3"]);
    }

    #[test]
    fn builds_flatpak_command() {
        let bro = sample_bro(
            BroSource::Flatpak,
            LaunchRecipe::Command {
                program: "flatpak".to_string(),
                args_before_inputs: vec![
                    "run".to_string(),
                    "--file-forwarding".to_string(),
                    "org.videolan.VLC".to_string(),
                    "@@".to_string(),
                ],
                args_after_inputs: vec!["@@".to_string()],
            },
        );
        let (program, args) = bro.build_command(&sample_inputs());
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

    #[test]
    fn preserves_wire_strings_for_source_and_protocol() {
        assert_eq!(BroSource::Native.as_str(), "native");
        assert_eq!(BroSource::Flatpak.as_str(), "flatpak");
        assert_eq!(BroProtocol::Fnf.as_str(), "fnf");
    }

    #[test]
    fn errors_when_launching_without_inputs() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "vlc".to_string(),
                args_before_inputs: vec![],
                args_after_inputs: vec![],
            },
        );
        let result = launch_bro(&[], &bro);
        assert!(result.is_err());
    }

    #[test]
    fn returns_empty_registry_result_on_unsupported_platforms() {
        #[cfg(not(target_os = "linux"))]
        assert!(list_bros(&sample_inputs()).is_empty());

        #[cfg(target_os = "linux")]
        {
            let _ = list_bros(&sample_inputs());
        }
    }
}
