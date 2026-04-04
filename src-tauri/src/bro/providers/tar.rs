use std::path::{Path, PathBuf};

use crate::bro::model::{
    BroOption, BroOutput, BroPre, BroProtocol, BroSource, LaunchArg, LaunchCwd, LaunchRecipe,
    ResolvedBro,
};
use crate::bro::probe::find_binary_on_path;
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::native_version;

pub struct TarProvider;

impl BroProvider for TarProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let Some(program) = find_binary_on_path(&["tar"]) else {
            return vec![];
        };

        vec![ResolvedBro {
            option: BroOption {
                id: "native:tar".to_string(),
                name: "tar".to_string(),
                version: native_version(&program),
                source: BroSource::Native,
                detail: Some(program.clone()),
                protocol: BroProtocol::Fp,
                output: Some(BroOutput {
                    label: "Archive path".to_string(),
                    default_value: default_output_path(ctx),
                }),
                params: vec![],
                pre: BroPre {
                    score: 0,
                    reason: String::new(),
                },
            },
            launch: LaunchRecipe::Command {
                program,
                cwd: Some(LaunchCwd::FirstInputParent),
                args: vec![
                    LaunchArg::Literal("-cf".to_string()),
                    LaunchArg::OutputPath,
                    LaunchArg::TargetNames,
                ],
            },
        }]
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
        return format!("{}.tar", ctx.inputs[0].name);
    }

    let base_name = first_input_parent(ctx.inputs)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "archive".to_string());
    format!("{base_name}.tar")
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

    fn single_input() -> Vec<BroInputItem> {
        vec![BroInputItem {
            path: "/tmp/source/movie.mkv".to_string(),
            name: "movie.mkv".to_string(),
            kind: "file".to_string(),
        }]
    }

    #[test]
    fn single_input_archive_name_uses_entry_name() {
        let inputs = single_input();
        let ctx = BroDiscoveryContext {
            inputs: &inputs,
            suggested_output_dir: Some("/tmp/out"),
        };
        assert_eq!(archive_file_name(&ctx), "movie.mkv.tar");
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
        assert_eq!(archive_file_name(&ctx), "source.tar");
    }

    #[test]
    fn suggested_output_dir_wins_for_default_output_path() {
        let inputs = single_input();
        let ctx = BroDiscoveryContext {
            inputs: &inputs,
            suggested_output_dir: Some("/tmp/out"),
        };
        assert_eq!(
            default_output_path(&ctx),
            Some("/tmp/out/movie.mkv.tar".to_string())
        );
    }
}
