//! Several Matroska files combined into one output by `mkvmerge` — e.g. the
//! video of one release with an audio track from another. Always a full
//! remux into a new file.

use crate::error::CoreError;
use crate::process::{ProcessOutput, ProcessRunner, ProgressSink};
use crate::remux::push_input_options;
use mkv_types::{MuxPlan, ProbeResult, TrackEdit};
use std::collections::HashSet;

/// Checks a plan against the inputs' current probe results before any
/// command line is built: every track must exist in its input with the same
/// UID and type, appear once, and every input must contribute a track. Like
/// `plan::validate`, this guards plans arriving over the HTTP API.
pub fn validate(originals: &[ProbeResult], plan: &MuxPlan) -> Result<(), CoreError> {
    let invalid = |message: String| Err(CoreError::InvalidPlan(message));
    if plan.inputs.is_empty() || plan.tracks.is_empty() {
        return invalid("a mux needs at least one input and one track".into());
    }
    if originals.len() != plan.inputs.len() {
        return invalid("one probe result per input is required".into());
    }

    let mut seen = HashSet::new();
    for track in &plan.tracks {
        let Some(original) = originals.get(track.input) else {
            return invalid(format!("track refers to input {}, which doesn't exist", track.input));
        };
        let exists = original
            .tracks
            .iter()
            .any(|t| t.id == track.edit.id && t.uid == track.edit.uid && t.track_type == track.edit.track_type);
        if !exists {
            return invalid(format!("track {} is not in input {}", track.edit.id, track.input));
        }
        if !seen.insert((track.input, track.edit.id)) {
            return invalid(format!("track {} of input {} appears twice", track.edit.id, track.input));
        }
    }

    for (i, (input, original)) in plan.inputs.iter().zip(originals).enumerate() {
        if !plan.tracks.iter().any(|t| t.input == i) {
            return invalid(format!("input {i} contributes no tracks"));
        }
        for removed in &input.remove_attachments {
            if !original.attachments.iter().any(|a| a.id == removed.id && a.uid == removed.uid) {
                return invalid(format!("attachment {} ({}) is not in input {i}", removed.id, removed.file_name));
            }
        }
    }
    Ok(())
}

/// Builds the `mkvmerge` argument list: per-input options followed by each
/// input's path, then the output track order across inputs. Only the first
/// input keeps its chapters and global tags; copying them from every file
/// would repeat them.
pub fn build_mkvmerge_args(plan: &MuxPlan) -> Vec<String> {
    let mut args = vec!["--gui-mode".to_string(), "-o".to_string(), plan.output_path.clone()];

    for (i, input) in plan.inputs.iter().enumerate() {
        let edits: Vec<TrackEdit> = plan.tracks.iter().filter(|t| t.input == i).map(|t| t.edit.clone()).collect();
        push_input_options(&mut args, &edits, &input.remove_attachments);
        if i > 0 {
            args.push("--no-chapters".to_string());
            args.push("--no-global-tags".to_string());
        }
        args.push(input.source_path.clone());
    }

    args.push("--track-order".to_string());
    let order = plan
        .tracks
        .iter()
        .map(|t| format!("{}:{}", t.input, t.edit.id))
        .collect::<Vec<_>>()
        .join(",");
    args.push(order);
    args
}

pub async fn run(
    runner: &dyn ProcessRunner,
    plan: &MuxPlan,
    on_progress: ProgressSink,
) -> Result<ProcessOutput, CoreError> {
    runner.run_mkvmerge(build_mkvmerge_args(plan), on_progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{AttachmentInfo, ContainerInfo, MuxInput, MuxTrack, TrackFlags, TrackInfo, TrackType};

    fn track(id: u64, uid: u64, track_type: TrackType) -> TrackInfo {
        TrackInfo { id, uid, track_type, codec: "codec".into(), language: None, name: None, flags: TrackFlags::default() }
    }

    fn kept(input: usize, t: &TrackInfo) -> MuxTrack {
        MuxTrack {
            input,
            edit: TrackEdit {
                id: t.id,
                uid: t.uid,
                track_type: t.track_type,
                name: Some(format!("in{input} t{}", t.id)),
                language: Some("en".into()),
                flags: TrackFlags::default(),
            },
        }
    }

    fn cover() -> AttachmentInfo {
        AttachmentInfo { id: 1, uid: 77, file_name: "cover.jpg".into(), mime_type: "image/jpeg".into(), size: 1 }
    }

    /// Input 0: video + English audio. Input 1: video + Telugu audio + subs.
    fn fixture() -> (Vec<ProbeResult>, MuxPlan) {
        let a = vec![track(0, 10, TrackType::Video), track(1, 11, TrackType::Audio)];
        let b = vec![track(0, 20, TrackType::Video), track(1, 21, TrackType::Audio), track(2, 22, TrackType::Subtitles)];
        let plan = MuxPlan {
            inputs: vec![
                MuxInput { source_path: "/a.mkv".into(), remove_attachments: vec![] },
                MuxInput { source_path: "/b.mkv".into(), remove_attachments: vec![cover()] },
            ],
            output_path: "/out.mkv".into(),
            tracks: vec![kept(0, &a[0]), kept(1, &b[1]), kept(0, &a[1]), kept(1, &b[2])],
        };
        let probe = |tracks| ProbeResult { container: ContainerInfo::default(), tracks, attachments: vec![] };
        let mut b_probe = probe(b);
        b_probe.attachments = vec![cover()];
        (vec![probe(a), b_probe], plan)
    }

    #[test]
    fn builds_per_input_selection_and_cross_input_order() {
        let (_, plan) = fixture();
        let args = build_mkvmerge_args(&plan);

        let a = args.iter().position(|x| x == "/a.mkv").unwrap();
        let b = args.iter().position(|x| x == "/b.mkv").unwrap();
        let (first, second) = (&args[..a], &args[a..b]);

        assert!(first.windows(2).any(|w| w == ["-d", "0"]));
        assert!(first.windows(2).any(|w| w == ["-a", "1"]));
        assert!(first.contains(&"-S".to_string()));
        assert!(!first.contains(&"--no-chapters".to_string()), "chapters come from the first input");

        assert!(second.contains(&"-D".to_string()), "input 1's video is not kept");
        assert!(second.windows(2).any(|w| w == ["-a", "1"]));
        assert!(second.windows(2).any(|w| w == ["-s", "2"]));
        assert!(second.windows(2).any(|w| w == ["--track-name", "1:in1 t1"]));
        assert!(second.windows(2).any(|w| w == ["--attachments", "!1"]));
        assert!(second.contains(&"--no-chapters".to_string()));
        assert!(second.contains(&"--no-global-tags".to_string()));

        assert!(args.windows(2).any(|w| w == ["--track-order", "0:0,1:1,0:1,1:2"]));
    }

    #[test]
    fn validation() {
        let (originals, plan) = fixture();
        assert!(validate(&originals, &plan).is_ok());

        let mut wrong_uid = plan.clone();
        wrong_uid.tracks[1].edit.uid = 999;
        assert!(validate(&originals, &wrong_uid).is_err());

        let mut duplicate = plan.clone();
        duplicate.tracks.push(duplicate.tracks[0].clone());
        assert!(validate(&originals, &duplicate).is_err());

        let mut unused = plan.clone();
        unused.tracks.retain(|t| t.input == 0);
        assert!(validate(&originals, &unused).unwrap_err().to_string().contains("contributes no tracks"));

        let mut bad_input = plan.clone();
        bad_input.tracks[0].input = 5;
        assert!(validate(&originals, &bad_input).is_err());

        let mut forged_attachment = plan;
        forged_attachment.inputs[0].remove_attachments = vec![cover()];
        assert!(validate(&originals, &forged_attachment).is_err());
    }
}
