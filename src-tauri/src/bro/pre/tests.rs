use super::rank_bros;
use crate::bro::model::{
    BroInputItem, BroOption, BroPre, BroProtocol, BroSource, LaunchRecipe, ResolvedBro,
};

fn bro(name: &str) -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: format!("native:{name}"),
            name: name.to_string(),
            version: "unknown".to_string(),
            source: BroSource::Native,
            detail: None,
            protocol: if name == "tar" {
                BroProtocol::Fp
            } else {
                BroProtocol::Fnf
            },
            output: None,
            params: vec![],
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
        },
        launch: LaunchRecipe::Command {
            program: name.to_string(),
            cwd: None,
            args: vec![],
        },
    }
}

#[test]
fn media_bros_sort_above_tar_for_media_selection() {
    let inputs = vec![BroInputItem {
        path: "/tmp/movie.mkv".to_string(),
        name: "movie.mkv".to_string(),
        kind: "file".to_string(),
    }];
    let bros = rank_bros(&inputs, vec![bro("tar"), bro("mpv")]);
    assert_eq!(bros[0].option.name, "mpv");
    assert!(bros[0].option.pre.score > bros[1].option.pre.score);
}

#[test]
fn tar_sorts_above_media_bros_for_folder_selection() {
    let inputs = vec![BroInputItem {
        path: "/tmp/source".to_string(),
        name: "source".to_string(),
        kind: "directory".to_string(),
    }];
    let bros = rank_bros(&inputs, vec![bro("mpv"), bro("tar")]);
    assert_eq!(bros[0].option.name, "tar");
    assert_eq!(bros[0].option.pre.reason, "Strong fit for bundling files and folders");
}

#[test]
fn seven_zip_sorts_above_tar_for_archive_selection() {
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
    let bros = rank_bros(&inputs, vec![bro("tar"), bro("7z")]);
    assert_eq!(bros[0].option.name, "7z");
    assert!(bros[0].option.pre.score > bros[1].option.pre.score);
}
