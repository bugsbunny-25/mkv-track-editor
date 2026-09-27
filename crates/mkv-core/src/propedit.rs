use crate::error::CoreError;
use crate::process::{ProcessOutput, ProcessRunner};
use mkv_types::RemuxPlan;

/// Builds one `mkvpropedit` invocation targeting `target_path`, with a
/// repeated `--edit track:=<uid> -s ...` block per track. Track UID (not
/// mkvmerge's positional track id) is used because it's stable regardless of
/// track order.
pub fn build_args(target_path: &str, plan: &RemuxPlan) -> Vec<String> {
    let mut args = vec![target_path.to_string()];

    for edit in &plan.edits {
        args.push("--edit".to_string());
        args.push(format!("track:={}", edit.uid));

        args.push("-s".to_string());
        args.push(format!("name={}", edit.name.clone().unwrap_or_default()));

        args.push("-s".to_string());
        args.push(format!(
            "language-ietf={}",
            edit.language.clone().unwrap_or_else(|| "und".to_string())
        ));

        let f = &edit.flags;
        for (property, value) in [
            ("flag-default", f.default),
            ("flag-forced", f.forced),
            ("flag-original", f.original),
            ("flag-hearing-impaired", f.hearing_impaired),
            ("flag-visual-impaired", f.visual_impaired),
            ("flag-commentary", f.commentary),
        ] {
            args.push("-s".to_string());
            args.push(format!("{property}={}", u8::from(value)));
        }
    }

    args
}

/// Runs the metadata-only fast path: copies the source to the output path
/// (a plain byte copy, not a remux) when they differ, then edits the copy's
/// track headers in place with `mkvpropedit`.
pub async fn run(runner: &dyn ProcessRunner, plan: &RemuxPlan) -> Result<ProcessOutput, CoreError> {
    if plan.source_path != plan.output_path {
        tokio::fs::copy(&plan.source_path, &plan.output_path).await?;
    }
    let args = build_args(&plan.output_path, plan);
    runner.run_mkvpropedit(args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{TrackEdit, TrackFlags, TrackType};

    #[test]
    fn builds_edit_blocks_per_track() {
        let plan = RemuxPlan {
            source_path: "/in.mkv".into(),
            output_path: "/out.mkv".into(),
            kept_track_order: vec![0, 1],
            edits: vec![
                TrackEdit {
                    id: 0,
                    uid: 111,
                    track_type: TrackType::Video,
                    name: Some("Movie".into()),
                    language: None,
                    flags: TrackFlags { default: true, ..TrackFlags::default() },
                },
                TrackEdit {
                    id: 1,
                    uid: 222,
                    track_type: TrackType::Subtitles,
                    name: None,
                    language: Some("ja".into()),
                    flags: TrackFlags {
                        forced: true,
                        hearing_impaired: true,
                        ..TrackFlags::default()
                    },
                },
            ],
        };

        let args = build_args("/out.mkv", &plan);
        assert_eq!(args[0], "/out.mkv");

        // Split into per-track blocks so each flag is checked against the
        // right `--edit` selector, not just "somewhere in the args".
        let second = args.iter().position(|a| a == "track:=222").unwrap();
        let (block_a, block_b) = args.split_at(second);

        assert!(block_a.windows(2).any(|w| w == ["--edit", "track:=111"]));
        assert!(block_a.windows(2).any(|w| w == ["-s", "name=Movie"]));
        assert!(block_a.windows(2).any(|w| w == ["-s", "flag-default=1"]));
        assert!(block_a.windows(2).any(|w| w == ["-s", "flag-hearing-impaired=0"]));

        assert!(block_b.windows(2).any(|w| w == ["-s", "language-ietf=ja"]));
        assert!(block_b.windows(2).any(|w| w == ["-s", "flag-forced=1"]));
        assert!(block_b.windows(2).any(|w| w == ["-s", "flag-hearing-impaired=1"]));
        assert!(block_b.windows(2).any(|w| w == ["-s", "flag-original=0"]));
        assert!(block_b.windows(2).any(|w| w == ["-s", "flag-commentary=0"]));
        assert!(block_b.windows(2).any(|w| w == ["-s", "flag-visual-impaired=0"]));
    }

    #[tokio::test]
    async fn run_skips_copy_when_output_equals_source() {
        use crate::process::MockProcessRunner;

        let plan = RemuxPlan {
            source_path: "/does/not/exist.mkv".into(),
            output_path: "/does/not/exist.mkv".into(),
            kept_track_order: vec![0],
            edits: vec![TrackEdit {
                id: 0,
                uid: 1,
                track_type: TrackType::Video,
                name: None,
                language: None,
                flags: TrackFlags::default(),
            }],
        };
        let runner = MockProcessRunner::default();
        // Would fail on fs::copy if the (skip-copy) branch weren't taken,
        // since the source path doesn't exist.
        run(&runner, &plan).await.unwrap();
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }
}
