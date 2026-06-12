use super::{archive_stem, default_extract_dir, detect_archive, ArchiveKind, ExtractProvider};
use crate::bro::model::BroInputItem;
use crate::bro::providers::{BroDiscoveryContext, BroProvider};

fn input(path: &str, name: &str, kind: &str) -> BroInputItem {
    BroInputItem {
        path: path.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
    }
}

#[test]
fn detects_compound_tar_gz_before_tar() {
    assert!(matches!(
        detect_archive("backup.tar.gz"),
        Some(ArchiveKind::TarGz)
    ));
    assert!(matches!(detect_archive("backup.TGZ"), Some(ArchiveKind::TarGz)));
    assert!(matches!(detect_archive("backup.tar"), Some(ArchiveKind::Tar)));
    assert!(matches!(detect_archive("photos.ZIP"), Some(ArchiveKind::Zip)));
    assert!(matches!(detect_archive("data.7z"), Some(ArchiveKind::SevenZip)));
    assert!(detect_archive("notes.txt").is_none());
}

#[test]
fn stem_strips_archive_extensions() {
    assert_eq!(archive_stem("photos.zip", ArchiveKind::Zip), "photos");
    assert_eq!(archive_stem("data.7z", ArchiveKind::SevenZip), "data");
    assert_eq!(archive_stem("backup.tar", ArchiveKind::Tar), "backup");
    assert_eq!(archive_stem("backup.tar.gz", ArchiveKind::TarGz), "backup");
    assert_eq!(archive_stem("backup.tgz", ArchiveKind::TarGz), "backup");
}

#[test]
fn default_dir_uses_archive_parent_and_stem() {
    let inputs = vec![input("/home/u/photos.zip", "photos.zip", "file")];
    let ctx = BroDiscoveryContext {
        inputs: &inputs,
        suggested_output_dir: None,
        source_id: None,
    };
    assert_eq!(
        default_extract_dir(&ctx, ArchiveKind::Zip).as_deref(),
        Some("/home/u/photos")
    );
}

#[test]
fn emits_single_extract_bro_for_archive_cursor() {
    let inputs = vec![input("/home/u/photos.zip", "photos.zip", "file")];
    let ctx = BroDiscoveryContext {
        inputs: &inputs,
        suggested_output_dir: None,
        source_id: None,
    };
    let bros = ExtractProvider.list(&ctx);
    assert_eq!(bros.len(), 1);
    assert_eq!(bros[0].option.id, "native:extract-zip");
    assert_eq!(bros[0].option.name, "unzip");
}

#[test]
fn no_bro_for_non_archive_or_directory_or_multiselect() {
    let ctx_dir = BroDiscoveryContext {
        inputs: &[input("/home/u/stuff", "stuff", "directory")],
        suggested_output_dir: None,
        source_id: None,
    };
    assert!(ExtractProvider.list(&ctx_dir).is_empty());

    let ctx_txt = BroDiscoveryContext {
        inputs: &[input("/home/u/notes.txt", "notes.txt", "file")],
        suggested_output_dir: None,
        source_id: None,
    };
    assert!(ExtractProvider.list(&ctx_txt).is_empty());

    let multi = vec![
        input("/home/u/a.zip", "a.zip", "file"),
        input("/home/u/b.zip", "b.zip", "file"),
    ];
    let ctx_multi = BroDiscoveryContext {
        inputs: &multi,
        suggested_output_dir: None,
        source_id: None,
    };
    assert!(ExtractProvider.list(&ctx_multi).is_empty());
}
