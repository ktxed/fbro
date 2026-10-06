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
            protocol: if name == "tar" || name == "zip" || name == "7z" {
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
fn check_hashes_ranks_above_every_normal_fit() {
    let inputs = vec![BroInputItem {
        path: "/tmp/a.mp4".into(),
        name: "a.mp4".into(),
        kind: "file".into(),
    }];
    let mut hash_bro = bro("Check hashes");
    hash_bro.option.id = "builtin:check-hashes".into();
    let ranked = rank_bros(&inputs, vec![bro("mpv"), bro("7z"), bro("tar"), hash_bro]);
    assert_eq!(ranked[0].option.id, "builtin:check-hashes");
    assert!(ranked[0].option.pre.score > ranked[1].option.pre.score);
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
    assert_eq!(
        bros[0].option.pre.reason,
        "Strong fit for bundling files and folders"
    );
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

#[test]
fn zip_sorts_above_tar_for_archive_selection() {
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
    let bros = rank_bros(&inputs, vec![bro("tar"), bro("zip")]);
    assert_eq!(bros[0].option.name, "zip");
    assert!(bros[0].option.pre.score > bros[1].option.pre.score);
}

#[test]
fn extract_bro_sorts_above_pack_bros_for_single_archive() {
    let inputs = vec![BroInputItem {
        path: "/tmp/photos.zip".to_string(),
        name: "photos.zip".to_string(),
        kind: "file".to_string(),
    }];
    let bros = rank_bros(&inputs, vec![bro("zip"), bro("7z"), bro("unzip")]);
    assert_eq!(bros[0].option.name, "unzip");
    assert_eq!(bros[0].option.pre.score, 99);
    assert!(bros[0].option.pre.score > bros[1].option.pre.score);
}

#[test]
fn generation_ranks_below_checker_and_strong_media_fit() {
    let inputs = vec![BroInputItem {
        path: "/tmp/movie.mp4".into(),
        name: "movie.mp4".into(),
        kind: "file".into(),
    }];
    let mut generate = bro("Generate hashes");
    generate.option.id = "builtin:generate-hashes".into();
    let mut check = bro("Check hashes");
    check.option.id = "builtin:check-hashes".into();
    let ranked = rank_bros(&inputs, vec![generate, check, bro("mpv")]);
    assert_eq!(ranked[0].option.id, "builtin:check-hashes");
    assert_eq!(ranked[1].option.name, "mpv");
    assert_eq!(ranked[2].option.id, "builtin:generate-hashes");
    assert_eq!(ranked[2].option.pre.score, 96);
}
