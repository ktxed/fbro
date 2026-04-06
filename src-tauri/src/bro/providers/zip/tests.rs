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
    assert_eq!(archive_file_name(&ctx), "movie.mkv.zip");
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
    assert_eq!(archive_file_name(&ctx), "source.zip");
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
        Some("/tmp/out/movie.mkv.zip".to_string())
    );
}
