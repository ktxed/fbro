use std::collections::HashMap;
use std::process::Command;

use crate::bro::model::{BroInputItem, ResolvedBro};
use crate::bro::pre::rank_bros;
use crate::bro::providers::{builtin_providers, BroDiscoveryContext};
use crate::bro::BroCatalogEntry;

pub fn list_bros(
    inputs: &[BroInputItem],
    suggested_output_dir: Option<&str>,
    source_id: Option<&str>,
) -> Vec<ResolvedBro> {
    let ctx = BroDiscoveryContext {
        inputs,
        suggested_output_dir,
        source_id,
    };
    let bros = builtin_providers()
        .into_iter()
        .flat_map(|provider| provider.list(&ctx))
        .collect();
    rank_bros(inputs, bros)
}

/// Return the user-facing bro capabilities available in this installation.
///
/// Unlike `list_bros`, this is not scoped to a panel or selection. Multiple
/// launch variants (for example native and Flatpak VLC) intentionally collapse
/// to one help entry.
pub fn list_bro_catalog() -> Vec<BroCatalogEntry> {
    let discovered = list_bros(&[], None, None);
    let tar_available = discovered
        .iter()
        .any(|bro| bro.option.name.eq_ignore_ascii_case("tar"));
    let mut catalog = Vec::new();

    for bro in discovered {
        if let Some(entry) = catalog_entry_for_discovered(&bro.option.name) {
            catalog.push(entry);
        }
    }

    catalog.extend([
        catalog_entry(
            "builtin.compare_files",
            "Compare files",
            "Compare two text files side by side.",
        ),
        catalog_entry(
            "builtin.sync_bro",
            "Sync Bro",
            "Verify that local and remote folders contain byte-identical files using BLAKE3.",
        ),
        catalog_entry(
            "builtin:fedup",
            "Fedup Bro",
            "Find exact duplicate files in a global selection using BLAKE3.",
        ),
        catalog_entry(
            "builtin:kill-proc",
            "Kill (SIGKILL)",
            "Force-terminate selected processes without a grace period.",
        ),
        catalog_entry(
            "native:extract-zip",
            "unzip",
            "Extract a ZIP archive into a folder.",
        ),
        catalog_entry(
            "native:extract-7z",
            "un7z",
            "Extract a 7z archive into a folder.",
        ),
    ]);

    if tar_available {
        catalog.extend([
            catalog_entry(
                "native:extract-tar",
                "untar",
                "Extract a TAR archive into a folder.",
            ),
            catalog_entry(
                "native:extract-targz",
                "untar.gz",
                "Extract a compressed TAR archive into a folder.",
            ),
        ]);
    }

    sort_and_dedup_catalog(&mut catalog);
    catalog
}

fn sort_and_dedup_catalog(catalog: &mut Vec<BroCatalogEntry>) {
    catalog.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    catalog.dedup_by(|left, right| left.id == right.id);
}

fn catalog_entry_for_discovered(name: &str) -> Option<BroCatalogEntry> {
    match name.to_ascii_lowercase().as_str() {
        "copy paths" => Some(catalog_entry(
            "builtin:copy-paths",
            "Copy Paths",
            "Copy the selected file and folder paths to the clipboard.",
        )),
        "mpv" => Some(catalog_entry(
            "media:mpv",
            "mpv",
            "Play the selected media files with mpv.",
        )),
        "vlc" => Some(catalog_entry(
            "media:vlc",
            "VLC",
            "Play the selected media files with VLC.",
        )),
        "7z" => Some(catalog_entry(
            "native:7z",
            "7z",
            "Create a 7z archive from the selected files and folders.",
        )),
        "tar" => Some(catalog_entry(
            "native:tar",
            "tar",
            "Create a TAR archive from the selected files and folders.",
        )),
        "zip" => Some(catalog_entry(
            "native:zip",
            "zip",
            "Create a ZIP archive from the selected files and folders.",
        )),
        _ => None,
    }
}

fn catalog_entry(id: &str, name: &str, description: &str) -> BroCatalogEntry {
    BroCatalogEntry {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
    }
}

pub fn launch_bro(
    inputs: &[BroInputItem],
    bro: &ResolvedBro,
    output_path: Option<&str>,
    params: &HashMap<String, String>,
) -> Result<std::process::Child, String> {
    if inputs.is_empty() {
        return Err("No input entries were resolved for this bro".to_string());
    }

    let (program, args, cwd) = bro.build_command(inputs, output_path, params)?;
    let mut cmd = Command::new(&program);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.spawn().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
