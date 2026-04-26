use crate::bro::model::{BroInputItem, BroOption, BroPre, ResolvedBro};

const SAMPLE_LIMIT: usize = 32;

pub fn rank_bros(inputs: &[BroInputItem], mut bros: Vec<ResolvedBro>) -> Vec<ResolvedBro> {
    for bro in &mut bros {
        bro.option.pre = evaluate_bro(inputs, &bro.option);
    }

    bros.sort_by(|left, right| {
        right
            .option
            .pre
            .score
            .cmp(&left.option.pre.score)
            .then_with(|| left.option.name.cmp(&right.option.name))
            .then_with(|| left.option.id.cmp(&right.option.id))
    });

    bros
}

fn evaluate_bro(inputs: &[BroInputItem], bro: &BroOption) -> BroPre {
    if is_mpv_bro(bro) {
        return evaluate_media_bro(inputs);
    }

    if is_vlc_bro(bro) {
        return nerf(evaluate_media_bro(inputs), 15);
    }

    if is_seven_zip_bro(bro) {
        return evaluate_seven_zip_bro(inputs);
    }

    if is_zip_bro(bro) {
        return evaluate_zip_bro(inputs);
    }

    if is_tar_bro(bro) {
        return evaluate_tar_bro(inputs);
    }

    BroPre {
        score: 50,
        reason: "Available for this selection".to_string(),
    }
}

fn is_mpv_bro(bro: &BroOption) -> bool {
    bro.name.eq_ignore_ascii_case("mpv")
}

fn is_vlc_bro(bro: &BroOption) -> bool {
    bro.name.eq_ignore_ascii_case("vlc")
}

fn nerf(pre: BroPre, penalty: i32) -> BroPre {
    BroPre {
        score: (pre.score - penalty).max(0),
        reason: pre.reason,
    }
}

fn is_tar_bro(bro: &BroOption) -> bool {
    bro.name.eq_ignore_ascii_case("tar")
}

fn is_seven_zip_bro(bro: &BroOption) -> bool {
    bro.name.eq_ignore_ascii_case("7z")
}

fn is_zip_bro(bro: &BroOption) -> bool {
    bro.name.eq_ignore_ascii_case("zip")
}

fn evaluate_media_bro(inputs: &[BroInputItem]) -> BroPre {
    let summary = summarize_inputs(inputs);
    if summary.total_inputs == 0 {
        return BroPre {
            score: 0,
            reason: "No input entries resolved".to_string(),
        };
    }

    if summary.sampled_files > 0 && summary.non_media_files == 0 {
        return BroPre {
            score: 100,
            reason: if summary.total_inputs == 1 {
                "Strong fit for this media file".to_string()
            } else {
                "Strong fit for this media selection".to_string()
            },
        };
    }

    if summary.media_files > 0 {
        return BroPre {
            score: 60,
            reason: "Mixed selection, but media inputs are present".to_string(),
        };
    }

    if summary.directory_inputs > 0 {
        return BroPre {
            score: 15,
            reason: "Directory-heavy selection; playable files are uncertain".to_string(),
        };
    }

    BroPre {
        score: 5,
        reason: "Weak fit for this selection".to_string(),
    }
}

fn evaluate_tar_bro(inputs: &[BroInputItem]) -> BroPre {
    let summary = summarize_inputs(inputs);
    if summary.total_inputs == 0 {
        return BroPre {
            score: 0,
            reason: "No input entries resolved".to_string(),
        };
    }

    if summary.directory_inputs > 0 || summary.total_inputs > 1 {
        return BroPre {
            score: 95,
            reason: "Strong fit for bundling files and folders".to_string(),
        };
    }

    BroPre {
        score: 35,
        reason: "Works, but archiving one file is a weaker fit".to_string(),
    }
}

fn evaluate_seven_zip_bro(inputs: &[BroInputItem]) -> BroPre {
    let summary = summarize_inputs(inputs);
    if summary.total_inputs == 0 {
        return BroPre {
            score: 0,
            reason: "No input entries resolved".to_string(),
        };
    }

    if summary.directory_inputs > 0 || summary.total_inputs > 1 {
        return BroPre {
            score: 98,
            reason: "Strong fit for archiving this selection".to_string(),
        };
    }

    BroPre {
        score: 72,
        reason: "Good fit for a single-file archive".to_string(),
    }
}

fn evaluate_zip_bro(inputs: &[BroInputItem]) -> BroPre {
    let summary = summarize_inputs(inputs);
    if summary.total_inputs == 0 {
        return BroPre {
            score: 0,
            reason: "No input entries resolved".to_string(),
        };
    }

    if summary.directory_inputs > 0 || summary.total_inputs > 1 {
        return BroPre {
            score: 96,
            reason: "Strong fit for a broadly compatible archive".to_string(),
        };
    }

    BroPre {
        score: 68,
        reason: "Good fit for a single-file archive".to_string(),
    }
}

struct InputSummary {
    total_inputs: usize,
    directory_inputs: usize,
    sampled_files: usize,
    media_files: usize,
    non_media_files: usize,
}

fn summarize_inputs(inputs: &[BroInputItem]) -> InputSummary {
    let mut summary = InputSummary {
        total_inputs: inputs.len(),
        directory_inputs: 0,
        sampled_files: 0,
        media_files: 0,
        non_media_files: 0,
    };

    for input in inputs.iter().take(SAMPLE_LIMIT) {
        if input.kind == "directory" {
            summary.directory_inputs += 1;
            continue;
        }

        summary.sampled_files += 1;
        if has_media_extension(&input.name) {
            summary.media_files += 1;
        } else {
            summary.non_media_files += 1;
        }
    }

    summary
}

fn has_media_extension(name: &str) -> bool {
    let extension = name
        .rsplit('.')
        .next()
        .filter(|ext| *ext != name)
        .map(|ext| ext.to_ascii_lowercase());
    let Some(extension) = extension.as_deref() else {
        return false;
    };

    matches!(
        extension,
        "3gp"
            | "aac"
            | "ac3"
            | "aif"
            | "aiff"
            | "avi"
            | "avif"
            | "bmp"
            | "flac"
            | "flv"
            | "gif"
            | "heic"
            | "jpeg"
            | "jpg"
            | "m4a"
            | "m4v"
            | "mka"
            | "mkv"
            | "mov"
            | "mp3"
            | "mp4"
            | "mpeg"
            | "mpg"
            | "ogg"
            | "ogm"
            | "ogv"
            | "opus"
            | "png"
            | "wav"
            | "webm"
            | "webp"
    )
}

#[cfg(test)]
mod tests;
