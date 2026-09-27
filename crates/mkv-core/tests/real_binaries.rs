//! Integration tests against the *real* `mkvmerge`/`mkvpropedit` binaries
//! (not the mock). These synthesize a tiny multi-track sample with `ffmpeg`
//! and are skipped (not failed) when either tool isn't on `PATH`, since
//! `ffmpeg` is a test-fixture convenience, not a project dependency.

use mkv_core::{analyze, execute_plan, probe, ExecutionStrategy, TokioProcessRunner, ToolPaths};
use mkv_types::{JobProgress, RemuxPlan, TrackEdit, TrackFlags, TrackType};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

async fn binary_available(bin: &str) -> bool {
    // ffmpeg uses a single-dash `-version`; mkvmerge/mkvpropedit use `--version`.
    let flag = if bin == "ffmpeg" { "-version" } else { "--version" };
    Command::new(bin)
        .arg(flag)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Builds a ~1s mkv with one video, one audio, and one subtitle track using
/// ffmpeg's built-in (no external library) encoders.
async fn make_sample(dir: &Path) -> PathBuf {
    let srt_path = dir.join("sample.srt");
    tokio::fs::write(
        &srt_path,
        "1\n00:00:00,000 --> 00:00:01,000\nHello\n",
    )
    .await
    .expect("write sample srt");

    let out_path = dir.join("sample.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x64:rate=5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
        ])
        .arg("-i")
        .arg(&srt_path)
        .args([
            "-c:v", "mpeg4",
            "-c:a", "aac",
            "-c:s", "srt",
            "-metadata:s:a:0", "language=eng",
            "-metadata:s:s:0", "language=eng",
        ])
        .arg(&out_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed to build sample fixture");
    out_path
}

#[tokio::test]
async fn round_trips_reorder_and_removal_through_real_mkvmerge() {
    if !binary_available("mkvmerge").await || !binary_available("ffmpeg").await {
        eprintln!("skipping: mkvmerge or ffmpeg not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let sample = make_sample(tmp.path()).await;
    let runner = TokioProcessRunner::new(ToolPaths::default());

    let original = probe(&runner, &sample).await.expect("probe sample");
    assert_eq!(original.tracks.len(), 3, "expected video+audio+subtitle tracks");

    let video = original
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Video)
        .unwrap();
    let audio = original
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Audio)
        .unwrap();
    let subs = original
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Subtitles)
        .unwrap();

    // Drop the subtitle track and reorder so audio comes before video.
    let output = tmp.path().join("out.mkv");
    let plan = RemuxPlan {
        source_path: sample.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        kept_track_order: vec![audio.id, video.id],
        edits: vec![
            TrackEdit {
                id: audio.id,
                uid: audio.uid,
                track_type: TrackType::Audio,
                name: Some("Renamed Audio".into()),
                language: Some("en".into()),
                flags: TrackFlags {
                    default: true,
                    original: true,
                    commentary: true,
                    visual_impaired: true,
                    ..TrackFlags::default()
                },
            },
            TrackEdit {
                id: video.id,
                uid: video.uid,
                track_type: TrackType::Video,
                name: video.name.clone(),
                language: video.language.clone(),
                flags: video.flags,
            },
        ],
    };
    let _ = subs; // only used to assert original track count above

    assert_eq!(
        mkv_core::plan::classify(&original, &plan),
        ExecutionStrategy::Remux
    );

    let outcome = execute_plan(&runner, &original, &plan, Box::new(|_p: JobProgress| {}))
        .await
        .expect("remux should succeed");
    assert_eq!(outcome.output_path, plan.output_path);

    let result = probe(&runner, &output).await.expect("probe output");
    assert_eq!(result.tracks.len(), 2, "subtitle track should have been dropped");
    assert_eq!(result.tracks[0].track_type, TrackType::Audio, "audio should now be first");
    assert_eq!(result.tracks[0].name.as_deref(), Some("Renamed Audio"));
    let flags = result.tracks[0].flags;
    assert!(flags.default && flags.original && flags.commentary && flags.visual_impaired);
    assert!(!flags.forced && !flags.hearing_impaired);
    assert_eq!(result.tracks[1].track_type, TrackType::Video);
}

#[tokio::test]
async fn metadata_only_edit_takes_mkvpropedit_fast_path() {
    if !binary_available("mkvmerge").await
        || !binary_available("mkvpropedit").await
        || !binary_available("ffmpeg").await
    {
        eprintln!("skipping: mkvmerge/mkvpropedit/ffmpeg not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let sample = make_sample(tmp.path()).await;
    let runner = TokioProcessRunner::new(ToolPaths::default());

    let original = probe(&runner, &sample).await.expect("probe sample");
    let edits: Vec<TrackEdit> = original
        .tracks
        .iter()
        .map(|t| TrackEdit {
            id: t.id,
            uid: t.uid,
            track_type: t.track_type,
            name: if t.track_type == TrackType::Video {
                Some("Renamed Video".into())
            } else {
                t.name.clone()
            },
            language: t.language.clone(),
            flags: if t.track_type == TrackType::Subtitles {
                TrackFlags { forced: true, hearing_impaired: true, ..t.flags }
            } else {
                t.flags
            },
        })
        .collect();
    let output = tmp.path().join("out.mkv");
    let plan = RemuxPlan {
        source_path: sample.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        kept_track_order: original.tracks.iter().map(|t| t.id).collect(),
        edits,
    };

    assert_eq!(
        mkv_core::plan::classify(&original, &plan),
        ExecutionStrategy::MetadataOnly
    );

    execute_plan(&runner, &original, &plan, Box::new(|_p: JobProgress| {}))
        .await
        .expect("metadata-only edit should succeed");

    let result = probe(&runner, &output).await.expect("probe output");
    assert_eq!(result.tracks.len(), 3, "no tracks should be dropped");
    let renamed_video = result
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Video)
        .unwrap();
    assert_eq!(renamed_video.name.as_deref(), Some("Renamed Video"));

    let subs = result
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Subtitles)
        .unwrap();
    assert!(subs.flags.forced && subs.flags.hearing_impaired, "flags set via mkvpropedit");
}

#[tokio::test]
async fn analyze_joins_mkvmerge_and_real_mediainfo_by_uid() {
    if !binary_available("mkvmerge").await
        || !binary_available("mediainfo").await
        || !binary_available("ffmpeg").await
    {
        eprintln!("skipping: mkvmerge/mediainfo/ffmpeg not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let sample = make_sample(tmp.path()).await;
    let runner = TokioProcessRunner::new(ToolPaths::default());

    let analysis = analyze(&runner, &sample).await.expect("analyze sample");
    assert_eq!(analysis.file_name, "sample.mkv");
    assert_eq!(analysis.probe.tracks.len(), 3);
    for track in &analysis.probe.tracks {
        let media = analysis
            .media
            .track(track.uid)
            .unwrap_or_else(|| panic!("MediaInfo has no track with uid {}", track.uid));
        assert_eq!(media.kind, Some(track.track_type));
    }
    let video = analysis
        .probe
        .tracks
        .iter()
        .find(|t| t.track_type == TrackType::Video)
        .unwrap();
    assert_eq!(analysis.media.track(video.uid).unwrap().width, Some(64));

    let titles: Vec<_> = analysis.sections.iter().map(|s| s.title.as_str()).collect();
    assert!(titles.contains(&"General") && titles.contains(&"Video") && titles.contains(&"Audio"));
    let complete_name = analysis.sections[0]
        .rows
        .iter()
        .find(|r| r.key == "Complete name")
        .unwrap();
    assert_eq!(complete_name.value, "sample.mkv", "absolute path must not leak");
}
