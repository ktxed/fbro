use super::{launch_bro, list_bros};
use crate::bro::model::{
    BroInputItem, BroLaunchInputKind, BroOption, BroOutput, BroParam, BroPre, BroProtocol,
    BroSource, LaunchArg, LaunchCwd, LaunchRecipe, ResolvedBro,
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
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
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
            cwd: None,
            args: vec![LaunchArg::Literal("--".to_string()), LaunchArg::TargetPaths],
        },
    );
    let (program, args, cwd) = bro
        .build_command(&sample_inputs(), None, &HashMap::new())
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
        .build_command(&sample_inputs(), None, &HashMap::new())
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
        .build_command(&sample_inputs(), None, &HashMap::new())
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
            output: Some(BroOutput {
                label: "Archive path".to_string(),
                default_value: Some("/tmp/out/archive.tar".to_string()),
            }),
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        launch: LaunchRecipe::Command {
            program: "tar".to_string(),
            cwd: Some(LaunchCwd::FirstInputParent),
            args: vec![
                LaunchArg::Literal("-cf".to_string()),
                LaunchArg::OutputPath,
                LaunchArg::TargetNames,
            ],
        },
    };
    let (program, args, cwd) = bro
        .build_command(
            &sample_inputs(),
            Some("/tmp/out/archive.tar"),
            &HashMap::new(),
        )
        .unwrap();
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
    let result = launch_bro(&[], &bro, None, &HashMap::new());
    assert!(result.is_err());
}

#[test]
fn builds_seven_zip_command_with_output_and_params() {
    let bro = ResolvedBro {
        option: BroOption {
            id: "native:7z".to_string(),
            name: "7z".to_string(),
            version: "unknown".to_string(),
            source: BroSource::Native,
            detail: None,
            protocol: BroProtocol::Fp,
            output: Some(BroOutput {
                label: "Archive path".to_string(),
                default_value: Some("/tmp/out/archive.7z".to_string()),
            }),
            params: vec![
                BroParam {
                    id: "compressionLevel".to_string(),
                    label: "Compression level".to_string(),
                    kind: BroLaunchInputKind::Int,
                    required: true,
                    default_value: Some("5".to_string()),
                    min_value: Some(0),
                    max_value: Some(9),
                },
                BroParam {
                    id: "password".to_string(),
                    label: "Password".to_string(),
                    kind: BroLaunchInputKind::Password,
                    required: false,
                    default_value: None,
                    min_value: None,
                    max_value: None,
                },
            ],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        launch: LaunchRecipe::Command {
            program: "7z".to_string(),
            cwd: Some(LaunchCwd::FirstInputParent),
            args: vec![
                LaunchArg::Literal("a".to_string()),
                LaunchArg::PrefixedParam {
                    id: "compressionLevel".to_string(),
                    prefix: "-mx".to_string(),
                },
                LaunchArg::OptionalPrefixedParam {
                    id: "password".to_string(),
                    prefix: "-p".to_string(),
                },
                LaunchArg::OutputPath,
                LaunchArg::Literal("--".to_string()),
                LaunchArg::TargetNames,
            ],
        },
    };
    let params = HashMap::from([
        ("compressionLevel".to_string(), "7".to_string()),
        ("password".to_string(), "hunter2".to_string()),
    ]);
    let (program, args, cwd) = bro
        .build_command(&sample_inputs(), Some("/tmp/out/archive.7z"), &params)
        .unwrap();
    assert_eq!(program, "7z");
    assert_eq!(
        args,
        vec![
            "a",
            "-mx7",
            "-phunter2",
            "/tmp/out/archive.7z",
            "--",
            "a.mp4",
            "b.mp3"
        ]
    );
    assert_eq!(
        cwd.map(|path| path.to_string_lossy().to_string()),
        Some("/tmp".to_string())
    );
}

#[test]
fn omits_optional_seven_zip_password_when_empty() {
    let bro = ResolvedBro {
        option: BroOption {
            id: "native:7z".to_string(),
            name: "7z".to_string(),
            version: "unknown".to_string(),
            source: BroSource::Native,
            detail: None,
            protocol: BroProtocol::Fp,
            output: Some(BroOutput {
                label: "Archive path".to_string(),
                default_value: Some("/tmp/out/archive.7z".to_string()),
            }),
            params: vec![BroParam {
                id: "compressionLevel".to_string(),
                label: "Compression level".to_string(),
                kind: BroLaunchInputKind::Int,
                required: true,
                default_value: Some("5".to_string()),
                min_value: Some(0),
                max_value: Some(9),
            }],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        launch: LaunchRecipe::Command {
            program: "7z".to_string(),
            cwd: Some(LaunchCwd::FirstInputParent),
            args: vec![
                LaunchArg::Literal("a".to_string()),
                LaunchArg::PrefixedParam {
                    id: "compressionLevel".to_string(),
                    prefix: "-mx".to_string(),
                },
                LaunchArg::OptionalPrefixedParam {
                    id: "password".to_string(),
                    prefix: "-p".to_string(),
                },
                LaunchArg::OutputPath,
                LaunchArg::Literal("--".to_string()),
                LaunchArg::TargetNames,
            ],
        },
    };
    let params = HashMap::from([("compressionLevel".to_string(), "5".to_string())]);
    let (_, args, _) = bro
        .build_command(&sample_inputs(), Some("/tmp/out/archive.7z"), &params)
        .unwrap();
    assert_eq!(
        args,
        vec!["a", "-mx5", "/tmp/out/archive.7z", "--", "a.mp4", "b.mp3"]
    );
}

#[test]
fn returns_empty_registry_result_on_unsupported_platforms() {
    #[cfg(target_os = "windows")]
    assert!(list_bros(&sample_inputs(), None, None).is_empty());

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let _ = list_bros(&sample_inputs(), None, None);
    }
}
