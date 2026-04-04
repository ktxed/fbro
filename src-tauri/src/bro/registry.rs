use std::collections::HashMap;
use std::process::Command;

use crate::bro::model::{BroInputItem, ResolvedBro};
use crate::bro::providers::{builtin_providers, BroDiscoveryContext};

pub fn list_bros(inputs: &[BroInputItem], suggested_output_dir: Option<&str>) -> Vec<ResolvedBro> {
    let ctx = BroDiscoveryContext {
        inputs,
        suggested_output_dir,
    };
    builtin_providers()
        .into_iter()
        .flat_map(|provider| provider.list(&ctx))
        .collect()
}

pub fn launch_bro(
    inputs: &[BroInputItem],
    bro: &ResolvedBro,
    launch_inputs: &HashMap<String, String>,
) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("No input entries were resolved for this bro".to_string());
    }

    let (program, args, cwd) = bro.build_command(inputs, launch_inputs)?;
    let mut cmd = Command::new(&program);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{launch_bro, list_bros};
    use crate::bro::model::{
        BroInputItem, BroLaunchInput, BroLaunchInputKind, BroOption, BroProtocol, BroSource,
        LaunchArg, LaunchCwd, LaunchRecipe, ResolvedBro,
    };
    use std::collections::HashMap;

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
                inputs: vec![],
            },
            launch,
            output_path_input_id: None,
        }
    }

    #[test]
    fn builds_native_mpv_command() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "mpv".to_string(),
                cwd: None,
                args: vec![LaunchArg::Literal("--".to_string()), LaunchArg::TargetPaths],
            },
        );
        let (program, args, cwd) = bro
            .build_command(&sample_inputs(), &HashMap::new())
            .unwrap();
        assert_eq!(program, "mpv");
        assert_eq!(args, vec!["--", "/tmp/a.mp4", "/tmp/b.mp3"]);
        assert_eq!(cwd, None);
    }

    #[test]
    fn builds_native_vlc_command() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "vlc".to_string(),
                cwd: None,
                args: vec![LaunchArg::TargetPaths],
            },
        );
        let (program, args, cwd) = bro
            .build_command(&sample_inputs(), &HashMap::new())
            .unwrap();
        assert_eq!(program, "vlc");
        assert_eq!(args, vec!["/tmp/a.mp4", "/tmp/b.mp3"]);
        assert_eq!(cwd, None);
    }

    #[test]
    fn builds_flatpak_command() {
        let bro = sample_bro(
            BroSource::Flatpak,
            LaunchRecipe::Command {
                program: "flatpak".to_string(),
                cwd: None,
                args: vec![
                    LaunchArg::Literal("run".to_string()),
                    LaunchArg::Literal("--file-forwarding".to_string()),
                    LaunchArg::Literal("org.videolan.VLC".to_string()),
                    LaunchArg::Literal("@@".to_string()),
                    LaunchArg::TargetPaths,
                    LaunchArg::Literal("@@".to_string()),
                ],
            },
        );
        let (program, args, cwd) = bro
            .build_command(&sample_inputs(), &HashMap::new())
            .unwrap();
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
        assert_eq!(cwd, None);
    }

    #[test]
    fn builds_tar_command_with_launch_input_and_input_names() {
        let bro = ResolvedBro {
            option: BroOption {
                id: "native:tar".to_string(),
                name: "tar".to_string(),
                version: "unknown".to_string(),
                source: BroSource::Native,
                detail: None,
                protocol: BroProtocol::Fp,
                inputs: vec![BroLaunchInput {
                    id: "outputPath".to_string(),
                    label: "Archive path".to_string(),
                    kind: BroLaunchInputKind::Path,
                    required: true,
                    default_value: Some("/tmp/out/archive.tar".to_string()),
                }],
            },
            launch: LaunchRecipe::Command {
                program: "tar".to_string(),
                cwd: Some(LaunchCwd::FirstInputParent),
                args: vec![
                    LaunchArg::Literal("-cf".to_string()),
                    LaunchArg::UserInput("outputPath".to_string()),
                    LaunchArg::TargetNames,
                ],
            },
            output_path_input_id: Some("outputPath".to_string()),
        };
        let launch_inputs =
            HashMap::from([("outputPath".to_string(), "/tmp/out/archive.tar".to_string())]);
        let (program, args, cwd) = bro.build_command(&sample_inputs(), &launch_inputs).unwrap();
        assert_eq!(program, "tar");
        assert_eq!(args, vec!["-cf", "/tmp/out/archive.tar", "a.mp4", "b.mp3"]);
        assert_eq!(
            cwd.map(|path| path.to_string_lossy().to_string()),
            Some("/tmp".to_string())
        );
    }

    #[test]
    fn preserves_wire_strings_for_source_and_protocol() {
        assert_eq!(BroSource::Native.as_str(), "native");
        assert_eq!(BroSource::Flatpak.as_str(), "flatpak");
        assert_eq!(BroProtocol::Fnf.as_str(), "fnf");
        assert_eq!(BroProtocol::Fp.as_str(), "fp");
    }

    #[test]
    fn errors_when_launching_without_inputs() {
        let bro = sample_bro(
            BroSource::Native,
            LaunchRecipe::Command {
                program: "vlc".to_string(),
                cwd: None,
                args: vec![LaunchArg::TargetPaths],
            },
        );
        let result = launch_bro(&[], &bro, &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn returns_empty_registry_result_on_unsupported_platforms() {
        #[cfg(target_os = "windows")]
        assert!(list_bros(&sample_inputs(), None).is_empty());

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let _ = list_bros(&sample_inputs(), None);
        }
    }
}
