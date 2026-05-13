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
        source_id: None,
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
        source_id: None,
    };
    assert_eq!(archive_file_name(&ctx), "source.7z");
}

#[cfg(unix)]
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
        source_id: None,
    };
    assert_eq!(
        default_output_path(&ctx),
        Some("/tmp/out/movie.mkv.7z".to_string())
    );
}
