use std::path::{Path, PathBuf};

use crate::bro::model::{
    BroLaunchInputKind, BroOption, BroOutput, BroParam, BroPre, BroProtocol, BroSource,
    LaunchRecipe, ResolvedBro,
};
use crate::bro::providers::{BroDiscoveryContext, BroProvider};

pub struct SevenZipProvider;

impl BroProvider for SevenZipProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        vec![ResolvedBro {
            option: BroOption {
                id: "native:7z".to_string(),
                name: "7z".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Built-in 7z encoder".to_string()),
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
            launch: LaunchRecipe::BuiltInSevenZip,
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
mod tests;
