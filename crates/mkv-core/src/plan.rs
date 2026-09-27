use crate::error::CoreError;
use mkv_types::{ProbeResult, RemuxPlan};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStrategy {
    /// No reorder, no removal — only metadata changed. Route to mkvpropedit.
    MetadataOnly,
    /// Reorder and/or removal requested — needs a full mkvmerge remux.
    Remux,
}

/// Validates internal consistency of a plan before it's used to build any
/// command line: every id in `kept_track_order` must have exactly one
/// matching `TrackEdit`, and ids must be unique. This is boundary validation
/// for plans arriving over the HTTP API (or from the wasm client), not a
/// defense against the desktop UI, which constructs plans from a trusted
/// local probe result.
pub fn validate(plan: &RemuxPlan) -> Result<(), CoreError> {
    if plan.kept_track_order.is_empty() {
        return Err(CoreError::InvalidPlan("plan keeps no tracks".into()));
    }

    let mut seen = HashSet::new();
    for id in &plan.kept_track_order {
        if !seen.insert(*id) {
            return Err(CoreError::InvalidPlan(format!("track id {id} appears twice in kept_track_order")));
        }
    }

    let edit_ids: HashSet<u64> = plan.edits.iter().map(|e| e.id).collect();
    if edit_ids.len() != plan.edits.len() {
        return Err(CoreError::InvalidPlan("duplicate track id in edits".into()));
    }
    if edit_ids != seen {
        return Err(CoreError::InvalidPlan(
            "kept_track_order and edits must reference exactly the same set of track ids".into(),
        ));
    }

    Ok(())
}

/// Decides whether `plan` can take the fast `mkvpropedit` path or needs a
/// full `mkvmerge` remux, by comparing the plan's kept/ordered tracks against
/// the original probed track order: identical order and set means only
/// metadata changed.
pub fn classify(original: &ProbeResult, plan: &RemuxPlan) -> ExecutionStrategy {
    let original_ids: Vec<u64> = original.tracks.iter().map(|t| t.id).collect();
    if plan.kept_track_order == original_ids {
        ExecutionStrategy::MetadataOnly
    } else {
        ExecutionStrategy::Remux
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{ContainerInfo, TrackEdit, TrackFlags, TrackInfo, TrackType};

    fn track(id: u64, uid: u64, t: TrackType) -> TrackInfo {
        TrackInfo {
            id,
            uid,
            track_type: t,
            codec: "codec".into(),
            language: None,
            name: None,
            flags: TrackFlags::default(),
        }
    }

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

    fn probe_with(ids: &[(u64, u64, TrackType)]) -> ProbeResult {
        ProbeResult {
            container: ContainerInfo::default(),
            tracks: ids.iter().map(|(id, uid, t)| track(*id, *uid, *t)).collect(),
        }
    }

    #[test]
    fn identity_order_is_metadata_only() {
        let original = probe_with(&[(0, 10, TrackType::Video), (1, 11, TrackType::Audio)]);
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![0, 1],
            edits: vec![
                edit(0, 10, TrackType::Video),
                edit(1, 11, TrackType::Audio),
            ],
        };
        assert_eq!(classify(&original, &plan), ExecutionStrategy::MetadataOnly);
    }

    #[test]
    fn reordered_tracks_need_remux() {
        let original = probe_with(&[(0, 10, TrackType::Video), (1, 11, TrackType::Audio)]);
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![1, 0],
            edits: vec![
                edit(0, 10, TrackType::Video),
                edit(1, 11, TrackType::Audio),
            ],
        };
        assert_eq!(classify(&original, &plan), ExecutionStrategy::Remux);
    }

    #[test]
    fn removed_track_needs_remux() {
        let original = probe_with(&[(0, 10, TrackType::Video), (1, 11, TrackType::Audio)]);
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![0],
            edits: vec![edit(0, 10, TrackType::Video)],
        };
        assert_eq!(classify(&original, &plan), ExecutionStrategy::Remux);
    }

    #[test]
    fn validate_rejects_mismatched_edits() {
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![0, 1],
            edits: vec![edit(0, 10, TrackType::Video)],
        };
        assert!(validate(&plan).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![0, 0],
            edits: vec![edit(0, 10, TrackType::Video), edit(0, 10, TrackType::Video)],
        };
        assert!(validate(&plan).is_err());
    }

    #[test]
    fn validate_rejects_empty_plan() {
        let plan = RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![],
            edits: vec![],
        };
        assert!(validate(&plan).is_err());
    }
}
