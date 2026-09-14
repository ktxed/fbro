use std::path::{Path, PathBuf};

use crate::bro::model::{
    BroLaunchInputKind, BroOption, BroOutput, BroParam, BroPre, BroProtocol, BroSource, LaunchArg,
    LaunchRecipe, ResolvedBro,
};
use crate::bro::probe::find_binary_on_path;
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::native_version;

pub struct ExtractProvider;

#[derive(Clone, Copy)]
enum ArchiveKind {
    Zip,
    SevenZip,
    Tar,
    TarGz,
}

impl BroProvider for ExtractProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        // Extraction targets exactly one archive file (the cursor entry when nothing is selected).
        if ctx.inputs.len() != 1 {
            return vec![];
        }
        let input = &ctx.inputs[0];
        if input.kind == "directory" {
            return vec![];
        }
        let Some(kind) = detect_archive(&input.name) else {
            return vec![];
        };

        let output = Some(BroOutput {
            label: "Extract to".to_string(),
            default_value: default_extract_dir(ctx, kind),
        });

        let bro = match kind {
            ArchiveKind::Zip => ResolvedBro {
                option: BroOption {
                    id: "native:extract-zip".to_string(),
                    name: "unzip".to_string(),
                    version: "builtin".to_string(),
                    source: BroSource::Native,
                    detail: Some("Built-in zip extractor".to_string()),
                    protocol: BroProtocol::Fp,
                    output,
                    params: vec![password_param()],
                    pre: default_pre(),
                },
                launch: LaunchRecipe::BuiltInZipExtract,
            },
            ArchiveKind::SevenZip => ResolvedBro {
                option: BroOption {
                    id: "native:extract-7z".to_string(),
                    name: "un7z".to_string(),
                    version: "builtin".to_string(),
                    source: BroSource::Native,
                    detail: Some("Built-in 7z extractor".to_string()),
                    protocol: BroProtocol::Fp,
                    output,
                    params: vec![password_param()],
                    pre: default_pre(),
                },
                launch: LaunchRecipe::BuiltInSevenZipExtract,
            },
            ArchiveKind::Tar | ArchiveKind::TarGz => {
                let Some(program) = find_binary_on_path(&["tar"]) else {
                    return vec![];
                };
                // `tar -xf` auto-detects gzip, so the recipe is shared between .tar and .tar.gz.
                let launch = LaunchRecipe::Command {
                    program: program.clone(),
                    cwd: None,
                    args: vec![
                        LaunchArg::Literal("-xf".to_string()),
                        LaunchArg::TargetPaths,
                        LaunchArg::Literal("-C".to_string()),
                        LaunchArg::OutputPath,
                    ],
                };
                let (id, name) = match kind {
                    ArchiveKind::TarGz => ("native:extract-targz", "untar.gz"),
                    _ => ("native:extract-tar", "untar"),
                };
                ResolvedBro {
                    option: BroOption {
                        id: id.to_string(),
                        name: name.to_string(),
                        version: native_version(&program),
                        source: BroSource::Native,
                        detail: Some(program),
                        protocol: BroProtocol::Fp,
                        output,
                        params: vec![],
                        pre: default_pre(),
                    },
                    launch,
                }
            }
        };

        vec![bro]
    }
}

fn detect_archive(name: &str) -> Option<ArchiveKind> {
    let lower = name.to_ascii_lowercase();
    // Check the compound tar.gz/.tgz suffixes before the bare .tar/.gz forms.
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return Some(ArchiveKind::TarGz);
    }
    if lower.ends_with(".tar") {
        return Some(ArchiveKind::Tar);
    }
    if lower.ends_with(".zip") {
        return Some(ArchiveKind::Zip);
    }
    if lower.ends_with(".7z") {
        return Some(ArchiveKind::SevenZip);
    }
    None
}

fn password_param() -> BroParam {
    BroParam {
        id: "password".to_string(),
        label: "Password".to_string(),
        kind: BroLaunchInputKind::Password,
        required: false,
        default_value: None,
        min_value: None,
        max_value: None,
    }
}

fn default_pre() -> BroPre {
    BroPre {
        score: 0,
        reason: String::new(),
    }
}

fn default_extract_dir(ctx: &BroDiscoveryContext<'_>, kind: ArchiveKind) -> Option<String> {
    let input = ctx.inputs.first()?;
    let stem = archive_stem(&input.name, kind);
    let output_dir = ctx
        .suggested_output_dir
        .map(PathBuf::from)
        .or_else(|| Path::new(&input.path).parent().map(Path::to_path_buf))?;
    Some(output_dir.join(stem).to_string_lossy().replace('\\', "/"))
}

/// Strip the archive extension(s) from `name` to produce a destination folder name.
fn archive_stem(name: &str, kind: ArchiveKind) -> String {
    let lower = name.to_ascii_lowercase();
    let suffix_len = match kind {
        ArchiveKind::Zip => 4,      // .zip
        ArchiveKind::SevenZip => 3, // .7z
        ArchiveKind::Tar => 4,      // .tar
        ArchiveKind::TarGz => {
            if lower.ends_with(".tar.gz") {
                7
            } else {
                4 // .tgz
            }
        }
    };
    let stem = &name[..name.len().saturating_sub(suffix_len)];
    if stem.is_empty() {
        name.to_string()
    } else {
        stem.to_string()
    }
}

#[cfg(test)]
mod tests;
