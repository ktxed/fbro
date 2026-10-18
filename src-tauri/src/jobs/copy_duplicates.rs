use std::collections::HashSet;
use std::path::Path;

use super::FileOpEntry;
use crate::vfs::VfsDriver;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuplicateNamePolicy {
    Exclude,
    Suffix,
}

impl DuplicateNamePolicy {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "exclude" => Ok(Self::Exclude),
            "suffix" => Ok(Self::Suffix),
            _ => Err(format!("Unknown repeated-filename policy: {value}")),
        }
    }
}

/// Resolve only top-level regular files. Directories keep their merge semantics.
/// Called in blocking work before constructing a job or dry-run script.
pub(crate) fn prepare_search_copy(
    files: Vec<FileOpEntry>,
    policy: DuplicateNamePolicy,
    dest_dir: &str,
    destination: &dyn VfsDriver,
) -> Result<Vec<FileOpEntry>, String> {
    prepare_with(
        files,
        policy,
        |name| destination.exists_no_follow(&destination.resolve_child(dest_dir, name)),
        || uuid::Uuid::new_v4().simple().to_string()[..4].to_string(),
    )
}

fn suffixed_name(name: &str, suffix: &str) -> String {
    let path = Path::new(name);
    match (
        path.file_stem().and_then(|s| s.to_str()),
        path.extension().and_then(|s| s.to_str()),
    ) {
        (Some(stem), Some(extension)) if !extension.is_empty() => {
            format!("{stem}-{suffix}.{extension}")
        }
        _ => format!("{name}-{suffix}"),
    }
}

fn prepare_with(
    files: Vec<FileOpEntry>,
    policy: DuplicateNamePolicy,
    mut exists: impl FnMut(&str) -> bool,
    mut suffix: impl FnMut() -> String,
) -> Result<Vec<FileOpEntry>, String> {
    // Reserve every original name, even ones occurring later in the batch.
    let mut reserved: HashSet<String> = files.iter().map(|file| file.name.clone()).collect();
    let mut seen = HashSet::new();
    let mut prepared = Vec::with_capacity(files.len());
    for mut file in files {
        if file.kind == "file" && !seen.insert(file.name.clone()) {
            if policy == DuplicateNamePolicy::Exclude {
                continue;
            }
            let mut destination_name = None;
            for _ in 0..128 {
                let candidate = suffixed_name(&file.name, &suffix());
                if !reserved.contains(&candidate) && !exists(&candidate) {
                    reserved.insert(candidate.clone());
                    destination_name = Some(candidate);
                    break;
                }
            }
            file.dest_name = Some(destination_name.ok_or_else(|| {
                format!(
                    "Could not find an unused name for {} after 128 attempts. Try copying again.",
                    file.name
                )
            })?);
        }
        prepared.push(file);
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, source: &str) -> FileOpEntry {
        FileOpEntry {
            name: name.into(),
            dest_name: None,
            source_path: source.into(),
            size: Some(3),
            kind: "file".into(),
            mtime: None,
            permissions: None,
        }
    }

    #[test]
    fn exclusion_keeps_first_source_and_does_not_merge_case_variants() {
        let files = vec![
            file("README.md", "/two/README.md"),
            file("README.md", "/one/README.md"),
            file("Readme.md", "/three/Readme.md"),
        ];
        let result = prepare_with(
            files,
            DuplicateNamePolicy::Exclude,
            |_| panic!("no destination lookup needed"),
            || panic!("no suffix needed"),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].source_path, "/two/README.md");
        assert_eq!(result[1].name, "Readme.md");
    }

    #[test]
    fn suffixes_keep_first_and_avoid_original_generated_and_existing_names() {
        let files = vec![
            file("README.md", "/one/README.md"),
            file("README.md", "/two/README.md"),
            file("README.md", "/three/README.md"),
            file("README-0000.md", "/four/README-0000.md"),
        ];
        let mut candidates = ["0000", "1111", "049f", "049f", "a217"].into_iter();
        let result = prepare_with(
            files,
            DuplicateNamePolicy::Suffix,
            |name| name == "README-1111.md",
            || candidates.next().unwrap().into(),
        )
        .unwrap();
        assert!(result[0].dest_name.is_none());
        assert_eq!(result[1].dest_name.as_deref(), Some("README-049f.md"));
        assert_eq!(result[2].dest_name.as_deref(), Some("README-a217.md"));
        assert_eq!(result[2].source_path, "/three/README.md");
        assert!(result[3].dest_name.is_none());
    }

    #[test]
    fn extension_and_dotfile_rules() {
        for (name, expected) in [
            ("README.md", "README-049f.md"),
            ("README", "README-049f"),
            (".env", ".env-049f"),
            (".env.local", ".env-049f.local"),
            ("archive.tar.gz", "archive.tar-049f.gz"),
            ("résumé.md", "résumé-049f.md"),
        ] {
            assert_eq!(suffixed_name(name, "049f"), expected);
        }
    }

    #[test]
    fn folders_and_symlinks_are_neither_excluded_nor_renamed() {
        for policy in [DuplicateNamePolicy::Exclude, DuplicateNamePolicy::Suffix] {
            let mut files = vec![
                file("same", "/one/same"),
                file("same", "/two/same"),
                file("same", "/three/same"),
                file("same", "/four/same"),
            ];
            files[0].kind = "directory".into();
            files[1].kind = "directory".into();
            files[2].kind = "symlink".into();
            let result = prepare_with(files, policy, |_| false, || "049f".into()).unwrap();
            assert_eq!(result.len(), 4);
            assert!(result.iter().all(|f| f.dest_name.is_none()));
        }
    }

    #[test]
    fn exhausted_suffixes_fail_without_overwriting() {
        let mut attempts = 0;
        let error = prepare_with(
            vec![file("same", "/one/same"), file("same", "/two/same")],
            DuplicateNamePolicy::Suffix,
            |_| true,
            || {
                attempts += 1;
                "049f".into()
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 128);
        assert!(error.contains("Could not find an unused name"));
    }

    #[test]
    fn parses_both_policies_and_rejects_unknown_values() {
        assert_eq!(
            DuplicateNamePolicy::parse("exclude").unwrap(),
            DuplicateNamePolicy::Exclude
        );
        assert_eq!(
            DuplicateNamePolicy::parse("suffix").unwrap(),
            DuplicateNamePolicy::Suffix
        );
        assert!(DuplicateNamePolicy::parse("overwrite").is_err());
    }
}
