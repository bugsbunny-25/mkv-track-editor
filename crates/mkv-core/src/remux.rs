use crate::error::CoreError;
use crate::process::{ProcessOutput, ProcessRunner, ProgressSink};
use mkv_types::{RemuxPlan, TrackType};

/// Builds the full `mkvmerge` argument list for `plan`. Per-file track
/// selection (`-a/-d/-s` or their negations) and per-track metadata options
/// must precede the source file path on mkvmerge's command line, since they
/// attach to whichever input file follows.
pub fn build_mkvmerge_args(plan: &RemuxPlan) -> Vec<String> {
    let mut args = vec!["--gui-mode".to_string(), "-o".to_string(), plan.output_path.clone()];

    let ids_of = |t: TrackType| -> Vec<u64> {
        plan.edits
            .iter()
            .filter(|e| e.track_type == t)
            .map(|e| e.id)
            .collect()
    };

    push_track_selection(&mut args, "-d", "-D", &ids_of(TrackType::Video));
    push_track_selection(&mut args, "-a", "-A", &ids_of(TrackType::Audio));
    push_track_selection(&mut args, "-s", "-S", &ids_of(TrackType::Subtitles));

    for edit in &plan.edits {
        args.push("--track-name".to_string());
        args.push(format!("{}:{}", edit.id, edit.name.clone().unwrap_or_default()));

        args.push("--language".to_string());
        args.push(format!(
            "{}:{}",
            edit.id,
            edit.language.clone().unwrap_or_else(|| "und".to_string())
        ));

        let f = &edit.flags;
        for (option, value) in [
            ("--default-track-flag", f.default),
            ("--forced-display-flag", f.forced),
            ("--original-flag", f.original),
            ("--hearing-impaired-flag", f.hearing_impaired),
            ("--visual-impaired-flag", f.visual_impaired),
            ("--commentary-flag", f.commentary),
        ] {
            args.push(option.to_string());
            args.push(format!("{}:{}", edit.id, bool_flag(value)));
        }
    }

    args.push("--track-order".to_string());
    let order = plan
        .kept_track_order
        .iter()
        .map(|id| format!("0:{id}"))
        .collect::<Vec<_>>()
        .join(",");
    args.push(order);

    // Every attachment not listed is copied, so fonts for styled subtitles
    // survive while cover images are dropped.
    if !plan.remove_attachments.is_empty() {
        args.push("--attachments".to_string());
        let ids: Vec<String> = plan.remove_attachments.iter().map(|a| a.id.to_string()).collect();
        args.push(format!("!{}", ids.join(",")));
    }

    args.push(plan.source_path.clone());
    args
}

fn push_track_selection(args: &mut Vec<String>, keep_flag: &str, none_flag: &str, ids: &[u64]) {
    if ids.is_empty() {
        // No tracks of this type are kept — either the source had none, or
        // they were all removed. `-D`/`-A`/`-S` is correct either way.
        args.push(none_flag.to_string());
    } else {
        args.push(keep_flag.to_string());
        args.push(ids.iter().map(u64::to_string).collect::<Vec<_>>().join(","));
    }
}

fn bool_flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

/// Runs a full mkvmerge remux for `plan`, forwarding progress percentages to
/// `on_progress` as they're parsed from `--gui-mode` output.
pub async fn run(
    runner: &dyn ProcessRunner,
    plan: &RemuxPlan,
    on_progress: ProgressSink,
) -> Result<ProcessOutput, CoreError> {
    let args = build_mkvmerge_args(plan);
    runner.run_mkvmerge(args, on_progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{AttachmentInfo, TrackEdit, TrackFlags};

    fn edit(id: u64, uid: u64, t: TrackType) -> TrackEdit {
        TrackEdit {
            id,
            uid,
            track_type: t,
            name: None,
            language: None,
            flags: TrackFlags::default(),
        }
    }

    #[test]
    fn builds_reorder_and_removal_args() {
        // Source has video(0), audio(1), audio(2); keep+reorder to 2,0 and drop 1.
        let plan = RemuxPlan {
            source_path: "/in.mkv".into(),
            output_path: "/out.mkv".into(),
            kept_track_order: vec![2, 0],
            edits: vec![
                edit(0, 10, TrackType::Video),
                edit(2, 12, TrackType::Audio),
            ],
            remove_attachments: vec![],
        };
        let args = build_mkvmerge_args(&plan);

        assert_eq!(args[0], "--gui-mode");
        assert_eq!(args[1], "-o");
        assert_eq!(args[2], "/out.mkv");
        // video kept: id 0
        assert!(args.windows(2).any(|w| w == ["-d", "0"]));
        // audio kept: id 2 (id 1 was dropped)
        assert!(args.windows(2).any(|w| w == ["-a", "2"]));
        // no subtitles at all -> -S
        assert!(args.contains(&"-S".to_string()));
        assert!(args.windows(2).any(|w| w == ["--track-order", "0:2,0:0"]));
        assert_eq!(args.last().unwrap(), "/in.mkv");
    }

    #[test]
    fn sets_metadata_flags_per_track() {
        let plan = RemuxPlan {
            source_path: "/in.mkv".into(),
            output_path: "/out.mkv".into(),
            kept_track_order: vec![1],
            edits: vec![TrackEdit {
                id: 1,
                uid: 99,
                track_type: TrackType::Audio,
                name: Some("Director's Commentary".into()),
                language: Some("en".into()),
                flags: TrackFlags {
                    default: true,
                    original: true,
                    commentary: true,
                    ..TrackFlags::default()
                },
            }],
            remove_attachments: vec![],
        };
        let args = build_mkvmerge_args(&plan);

        assert!(args
            .windows(2)
            .any(|w| w == ["--track-name", "1:Director's Commentary"]));
        assert!(args.windows(2).any(|w| w == ["--language", "1:en"]));
        assert!(args.windows(2).any(|w| w == ["--default-track-flag", "1:1"]));
        assert!(args.windows(2).any(|w| w == ["--forced-display-flag", "1:0"]));
        assert!(args.windows(2).any(|w| w == ["--original-flag", "1:1"]));
        assert!(args.windows(2).any(|w| w == ["--hearing-impaired-flag", "1:0"]));
        assert!(args.windows(2).any(|w| w == ["--visual-impaired-flag", "1:0"]));
        assert!(args.windows(2).any(|w| w == ["--commentary-flag", "1:1"]));

        // Every per-track option must precede the source path it applies to.
        let src_pos = args.iter().position(|a| a == "/in.mkv").unwrap();
        let last_flag = args.iter().rposition(|a| a == "--commentary-flag").unwrap();
        assert!(last_flag < src_pos);
    }

    #[test]
    fn drops_only_the_listed_attachments() {
        let cover = AttachmentInfo {
            id: 1,
            uid: 501,
            file_name: "cover.jpg".into(),
            mime_type: "image/jpeg".into(),
            size: 10,
        };
        let poster = AttachmentInfo { id: 3, uid: 503, file_name: "poster.png".into(), ..cover.clone() };
        let plan = RemuxPlan {
            source_path: "/in.mkv".into(),
            output_path: "/out.mkv".into(),
            kept_track_order: vec![0],
            edits: vec![edit(0, 10, TrackType::Video)],
            remove_attachments: vec![cover, poster],
        };
        let args = build_mkvmerge_args(&plan);
        assert!(args.windows(2).any(|w| w == ["--attachments", "!1,3"]));
        assert_eq!(args.last().unwrap(), "/in.mkv");

        let keep_all = RemuxPlan { remove_attachments: vec![], ..plan };
        assert!(!build_mkvmerge_args(&keep_all).contains(&"--attachments".to_string()));
    }

    #[tokio::test]
    async fn run_reports_progress_from_mock() {
        use crate::process::MockProcessRunner;
        use std::sync::{Arc, Mutex};

        let runner = MockProcessRunner {
            mkvmerge_progress_lines: vec![
                "#GUI#progress 0%".into(),
                "garbage line".into(),
                "#GUI#progress 50%".into(),
                "#GUI#progress 100%".into(),
            ],
            ..Default::default()
        };
        let plan = RemuxPlan {
            source_path: "/in.mkv".into(),
            output_path: "/out.mkv".into(),
            kept_track_order: vec![0],
            edits: vec![edit(0, 10, TrackType::Video)],
            remove_attachments: vec![],
        };

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        run(&runner, &plan, Box::new(move |pct| seen2.lock().unwrap().push(pct)))
            .await
            .unwrap();

        assert_eq!(*seen.lock().unwrap(), vec![0, 50, 100]);
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }
}
