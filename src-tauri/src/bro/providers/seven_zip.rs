use std::path::{Path, PathBuf};

use crate::bro::model::{
    BroLaunchInputKind, BroOption, BroOutput, BroParam, BroPre, BroProtocol, BroSource, LaunchArg,
    LaunchCwd, LaunchRecipe, ResolvedBro,
};
#[cfg(target_os = "macos")]
use crate::bro::probe::macos_common_cli_candidates;
use crate::bro::probe::{find_binary_on_path, resolve_existing_executable};
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::native_version;

pub struct SevenZipProvider;

impl BroProvider for SevenZipProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let Some(program) = find_seven_zip_binary() else {
            return vec![];
        };

        vec![ResolvedBro {
            option: BroOption {
                id: "native:7z".to_string(),
                name: "7z".to_string(),
                version: native_version(&program),
                source: BroSource::Native,
                detail: Some(program.clone()),
                protocol: BroProtocol::Fp,
                output: Some(BroOutput {
                    label: "Archive path".to_string(),
                    default_value: default_output_path(ctx),
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
                program,
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
        }]
    }
}

fn find_seven_zip_binary() -> Option<String> {
    find_binary_on_path(&["7z", "7zz", "7za", "7zr"]).or_else(|| {
        explicit_binary_candidates()
            .into_iter()
            .find_map(|path| resolve_existing_executable(&path))
    })
}

fn explicit_binary_candidates() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        return macos_common_cli_candidates(&["7z", "7zz", "7za", "7zr"]);
    }

    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

fn default_output_path(ctx: &BroDiscoveryContext<'_>) -> Option<String> {
    let file_name = archive_file_name(ctx);
    let output_dir = ctx
        .suggested_output_dir
        .map(PathBuf::from)
        .or_else(|| first_input_parent(ctx.inputs))?;
    Some(output_dir.join(file_name).to_string_lossy().to_string())
}

fn archive_file_name(ctx: &BroDiscoveryContext<'_>) -> String {
    if ctx.inputs.len() == 1 {
        return format!("{}.7z", ctx.inputs[0].name);
    }

    let base_name = first_input_parent(ctx.inputs)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "archive".to_string());
    format!("{base_name}.7z")
}

fn first_input_parent(inputs: &[crate::bro::model::BroInputItem]) -> Option<PathBuf> {
    let first = inputs.first()?;
    Path::new(&first.path).parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::{archive_file_name, default_output_path};
    use crate::bro::model::BroInputItem;
    use crate::bro::providers::BroDiscoveryContext;

    #[test]
    fn single_input_archive_name_uses_entry_name() {
        let inputs = vec![BroInputItem {
            path: "/tmp/source/movie.mkv".to_string(),
            name: "movie.mkv".to_string(),
            kind: "file".to_string(),
        }];
        let ctx = BroDiscoveryContext {
            inputs: &inputs,
            suggested_output_dir: Some("/tmp/out"),
        };
        assert_eq!(archive_file_name(&ctx), "movie.mkv.7z");
    }

    #[test]
    fn multiple_input_archive_name_uses_panel_directory_name() {
        let inputs = vec![
            BroInputItem {
                path: "/tmp/source/a.txt".to_string(),
                name: "a.txt".to_string(),
                kind: "file".to_string(),
            },
            BroInputItem {
                path: "/tmp/source/b.txt".to_string(),
                name: "b.txt".to_string(),
                kind: "file".to_string(),
            },
        ];
        let ctx = BroDiscoveryContext {
            inputs: &inputs,
            suggested_output_dir: Some("/tmp/out"),
        };
        assert_eq!(archive_file_name(&ctx), "source.7z");
    }

    #[test]
    fn suggested_output_dir_wins_for_default_output_path() {
        let inputs = vec![BroInputItem {
            path: "/tmp/source/movie.mkv".to_string(),
            name: "movie.mkv".to_string(),
            kind: "file".to_string(),
        }];
        let ctx = BroDiscoveryContext {
            inputs: &inputs,
            suggested_output_dir: Some("/tmp/out"),
        };
        assert_eq!(
            default_output_path(&ctx),
            Some("/tmp/out/movie.mkv.7z".to_string())
        );
    }

}
