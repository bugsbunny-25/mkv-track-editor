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

/// Checks that every attachment the plan removes exists in the probed source
/// with the same id and UID, so a stale or forged plan can't delete the
/// wrong attachment (mkvmerge selects by id, mkvpropedit by UID).
pub fn validate_attachments(original: &ProbeResult, plan: &RemuxPlan) -> Result<(), CoreError> {
    for removed in &plan.remove_attachments {
        let exists = original
            .attachments
            .iter()
            .any(|a| a.id == removed.id && a.uid == removed.uid);
        if !exists {
            return Err(CoreError::InvalidPlan(format!(
                "attachment {} ({}) is not in the source file",
                removed.id, removed.file_name
            )));
        }
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
    use mkv_types::{AttachmentInfo, ContainerInfo, TrackEdit, TrackFlags, TrackInfo, TrackType};

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

    fn attachment(id: u64, uid: u64) -> AttachmentInfo {
        AttachmentInfo {
            id,
            uid,
            file_name: "cover.jpg".into(),
            mime_type: "image/jpeg".into(),
            size: 1,
        }
    }

    #[test]
    fn removed_attachments_must_exist_in_the_source() {
        let mut original = probe_with(&[(0, 10, TrackType::Video)]);
        original.attachments = vec![attachment(1, 501)];
        let plan = |removed| RemuxPlan {
            source_path: "in.mkv".into(),
            output_path: "out.mkv".into(),
            kept_track_order: vec![0],
            edits: vec![edit(0, 10, TrackType::Video)],
            remove_attachments: vec![removed],
        };
        assert!(validate_attachments(&original, &plan(attachment(1, 501))).is_ok());
        assert!(validate_attachments(&original, &plan(attachment(1, 999))).is_err());
        assert!(validate_attachments(&original, &plan(attachment(2, 501))).is_err());
    }

    fn probe_with(ids: &[(u64, u64, TrackType)]) -> ProbeResult {
        ProbeResult {
            container: ContainerInfo::default(),
            tracks: ids.iter().map(|(id, uid, t)| track(*id, *uid, *t)).collect(),
            attachments: vec![],
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
            remove_attachments: vec![],
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
            remove_attachments: vec![],
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
            remove_attachments: vec![],
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
            remove_attachments: vec![],
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
            remove_attachments: vec![],
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
            remove_attachments: vec![],
        };
        assert!(validate(&plan).is_err());
    }
}
