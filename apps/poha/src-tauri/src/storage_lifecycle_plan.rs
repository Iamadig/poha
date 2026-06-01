use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::scan::{
    DELETED_DIR, DELETED_RETENTION_DAYS, TRANSCRIPT_CHUNKS_DIR, collect_session_dirs,
    deleted_session_time, file_name_string, metadata_string, path_string, read_json, scan_path,
    valid_audio_file,
};
use super::validation::{
    StorageCandidateKind, chunk_manifest_validation, transcript_validation, validation_facts,
};
use super::{StorageError, StorageMaintenanceAction, StorageMaintenanceSkip};

const CAPTURE_SCRATCH_AUDIO_FILES: &[&str] = &[
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
    "audio.m4a",
    "audio_mic.wav",
    "audio_mic_processed.wav",
    "audio_spk.wav",
];

pub(super) fn collect_actions(
    recordings_dir: &Path,
    now: DateTime<Utc>,
) -> Result<(Vec<StorageMaintenanceAction>, Vec<StorageMaintenanceSkip>), StorageError> {
    let mut actions = Vec::new();
    let mut skipped = Vec::new();
    let restored_artifacts = ledgered_artifact_paths(recordings_dir);

    for session_dir in collect_session_dirs(recordings_dir)? {
        add_session_actions(
            &session_dir,
            &restored_artifacts,
            &mut actions,
            &mut skipped,
        )?;
    }
    add_deleted_actions(recordings_dir, now, &mut actions, &mut skipped)?;

    Ok((actions, skipped))
}

pub(super) fn collect_actions_for_session(
    recordings_dir: &Path,
    session_dir: &Path,
) -> Result<(Vec<StorageMaintenanceAction>, Vec<StorageMaintenanceSkip>), StorageError> {
    let mut actions = Vec::new();
    let mut skipped = Vec::new();
    let restored_artifacts = ledgered_artifact_paths(recordings_dir);
    add_session_actions(session_dir, &restored_artifacts, &mut actions, &mut skipped)?;
    Ok((actions, skipped))
}

fn add_session_actions(
    session_dir: &Path,
    restored_artifacts: &BTreeSet<String>,
    actions: &mut Vec<StorageMaintenanceAction>,
    skipped: &mut Vec<StorageMaintenanceSkip>,
) -> Result<(), StorageError> {
    let chunks_dir = session_dir.join(TRANSCRIPT_CHUNKS_DIR);
    let stems = [
        session_dir.join("audio_mic.wav"),
        session_dir.join("audio_mic_processed.wav"),
        session_dir.join("audio_spk.wav"),
    ];
    let legacy_wav = session_dir.join("audio.wav");
    let metadata = read_json(&session_dir.join("session.json")).ok();
    let capture_dir = metadata
        .as_ref()
        .and_then(|value| metadata_string(value, "capture_dir"))
        .map(PathBuf::from);
    let has_candidates = chunks_dir.exists()
        || stems.iter().any(|path| path.exists())
        || legacy_wav.exists()
        || capture_dir
            .as_ref()
            .is_some_and(|dir| capture_scratch_audio_exists(session_dir, dir));
    if !has_candidates {
        return Ok(());
    }

    let session_id = metadata
        .as_ref()
        .and_then(|value| metadata_string(value, "id"))
        .unwrap_or_else(|| file_name_string(session_dir));
    if metadata
        .as_ref()
        .and_then(|value| metadata_string(value, "status"))
        .as_deref()
        != Some("done")
    {
        skipped.push(skip(&session_id, session_dir, "session is not finalized"));
        return Ok(());
    }
    let Some(transcript) = transcript_validation(session_dir) else {
        skipped.push(skip(
            &session_id,
            session_dir,
            "final transcript is missing or invalid",
        ));
        return Ok(());
    };

    let mp3_path = session_dir.join("audio.mp3");
    let valid_mp3 = valid_audio_file(&mp3_path);
    let will_have_valid_mp3 = valid_mp3 || legacy_wav.exists();
    let mixed_mp3_planned = will_have_valid_mp3;
    let chunk_manifest = chunk_manifest_validation(session_dir);
    if legacy_wav.exists() {
        if restored_artifacts.contains(&path_string(&legacy_wav)) {
            skipped.push(skip(
                &session_id,
                &legacy_wav,
                "artifact was restored after previous storage maintenance",
            ));
        } else {
            let kind = if valid_mp3 {
                StorageCandidateKind::LegacyWav
            } else {
                StorageCandidateKind::LegacyWavTranscode
            };
            match validation_facts(
                session_dir,
                metadata.as_ref(),
                &transcript,
                Some(&legacy_wav),
                kind,
                None,
                mixed_mp3_planned,
            ) {
                Ok(facts) => actions.push(action(
                    kind.as_str(),
                    &session_id,
                    &legacy_wav,
                    "finalized mixed WAV can be replaced by MP3",
                    Some(facts),
                )?),
                Err(reason) => skipped.push(skip(&session_id, &legacy_wav, &reason)),
            }
        }
    }
    if will_have_valid_mp3 && chunks_dir.exists() {
        if restored_artifacts.contains(&path_string(&chunks_dir)) {
            skipped.push(skip(
                &session_id,
                &chunks_dir,
                "artifact was restored after previous storage maintenance",
            ));
        } else if chunk_manifest.is_complete {
            match validation_facts(
                session_dir,
                metadata.as_ref(),
                &transcript,
                None,
                StorageCandidateKind::TranscriptChunks,
                Some(chunk_manifest.state.clone()),
                mixed_mp3_planned,
            ) {
                Ok(facts) => actions.push(action(
                    "transcriptChunks",
                    &session_id,
                    &chunks_dir,
                    "derived transcription chunks after final transcript",
                    Some(facts),
                )?),
                Err(reason) => skipped.push(skip(&session_id, &chunks_dir, &reason)),
            }
        } else {
            skipped.push(skip(&session_id, &chunks_dir, &chunk_manifest.state));
        }
    }
    if valid_mp3 {
        for stem in stems.iter().filter(|path| path.exists()) {
            if restored_artifacts.contains(&path_string(stem)) {
                skipped.push(skip(
                    &session_id,
                    stem,
                    "artifact was restored after previous storage maintenance",
                ));
                continue;
            }
            match validation_facts(
                session_dir,
                metadata.as_ref(),
                &transcript,
                Some(stem),
                StorageCandidateKind::StemWav,
                None,
                mixed_mp3_planned,
            ) {
                Ok(facts) => actions.push(action(
                    "stemWav",
                    &session_id,
                    stem,
                    "mic/system stem WAV after mixed MP3 and final transcript",
                    Some(facts),
                )?),
                Err(reason) => skipped.push(skip(&session_id, stem, &reason)),
            }
        }
        add_capture_scratch_actions(
            session_dir,
            metadata.as_ref(),
            &transcript,
            &session_id,
            &restored_artifacts,
            actions,
            skipped,
        )?;
    } else if stems.iter().any(|path| path.exists()) && legacy_wav.exists() {
        skipped.push(skip(
            &session_id,
            session_dir,
            "waiting for mixed MP3 audio quality validation",
        ));
    } else if chunks_dir.exists() || stems.iter().any(|path| path.exists()) {
        skipped.push(skip(
            &session_id,
            session_dir,
            "waiting for valid mixed MP3",
        ));
    }
    Ok(())
}

fn add_capture_scratch_actions(
    session_dir: &Path,
    metadata: Option<&Value>,
    transcript: &super::validation::TranscriptValidation,
    session_id: &str,
    restored_artifacts: &BTreeSet<String>,
    actions: &mut Vec<StorageMaintenanceAction>,
    skipped: &mut Vec<StorageMaintenanceSkip>,
) -> Result<(), StorageError> {
    let Some(capture_dir) = metadata
        .and_then(|value| metadata_string(value, "capture_dir"))
        .map(PathBuf::from)
    else {
        return Ok(());
    };
    if !capture_dir.is_dir() || same_path(&capture_dir, session_dir) {
        return Ok(());
    }

    for file_name in CAPTURE_SCRATCH_AUDIO_FILES {
        let scratch = capture_dir.join(file_name);
        if !scratch.exists() || same_path(&scratch, &session_dir.join(file_name)) {
            continue;
        }
        if restored_artifacts.contains(&path_string(&scratch)) {
            skipped.push(skip(
                session_id,
                &scratch,
                "artifact was restored after previous storage maintenance",
            ));
            continue;
        }
        match validation_facts(
            session_dir,
            metadata,
            transcript,
            Some(&scratch),
            StorageCandidateKind::CaptureScratchAudio,
            None,
            false,
        ) {
            Ok(facts) => actions.push(action_with_validation_dir(
                StorageCandidateKind::CaptureScratchAudio.as_str(),
                session_id,
                &scratch,
                "duplicate capture scratch audio after finalized archive",
                Some(facts),
                session_dir,
            )?),
            Err(reason) => skipped.push(skip(session_id, &scratch, &reason)),
        }
    }
    Ok(())
}

fn capture_scratch_audio_exists(session_dir: &Path, capture_dir: &Path) -> bool {
    capture_dir.is_dir()
        && !same_path(capture_dir, session_dir)
        && CAPTURE_SCRATCH_AUDIO_FILES.iter().any(|file_name| {
            let scratch = capture_dir.join(file_name);
            scratch.exists() && !same_path(&scratch, &session_dir.join(file_name))
        })
}

fn ledgered_artifact_paths(recordings_dir: &Path) -> BTreeSet<String> {
    let ledger_path = recordings_dir
        .join(".poha")
        .join("storage-maintenance.jsonl");
    let Ok(content) = fs::read_to_string(ledger_path) else {
        return BTreeSet::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            value
                .get("path")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn add_deleted_actions(
    recordings_dir: &Path,
    now: DateTime<Utc>,
    actions: &mut Vec<StorageMaintenanceAction>,
    skipped: &mut Vec<StorageMaintenanceSkip>,
) -> Result<(), StorageError> {
    let deleted_root = recordings_dir.join(DELETED_DIR);
    if !deleted_root.is_dir() {
        return Ok(());
    }
    let cutoff = now - Duration::days(DELETED_RETENTION_DAYS);
    for path in collect_session_dirs(&deleted_root)? {
        let deleted_at = deleted_session_time(&path)?;
        let id = file_name_string(&path);
        if deleted_at <= cutoff {
            actions.push(action(
                "deletedRecording",
                &id,
                &path,
                "soft-deleted recording older than retention window",
                None,
            )?);
        } else {
            skipped.push(skip(
                &id,
                &path,
                "soft-deleted recording is inside retention window",
            ));
        }
    }
    Ok(())
}

fn action(
    kind: &str,
    session_id: &str,
    path: &Path,
    reason: &str,
    validation_facts: Option<super::validation::StorageValidationFacts>,
) -> Result<StorageMaintenanceAction, StorageError> {
    action_inner(kind, session_id, path, reason, validation_facts, None)
}

fn action_with_validation_dir(
    kind: &str,
    session_id: &str,
    path: &Path,
    reason: &str,
    validation_facts: Option<super::validation::StorageValidationFacts>,
    validation_dir: &Path,
) -> Result<StorageMaintenanceAction, StorageError> {
    action_inner(
        kind,
        session_id,
        path,
        reason,
        validation_facts,
        Some(path_string(validation_dir)),
    )
}

fn action_inner(
    kind: &str,
    session_id: &str,
    path: &Path,
    reason: &str,
    validation_facts: Option<super::validation::StorageValidationFacts>,
    validation_dir: Option<String>,
) -> Result<StorageMaintenanceAction, StorageError> {
    Ok(StorageMaintenanceAction {
        kind: kind.to_string(),
        session_id: Some(session_id.to_string()),
        path: path_string(path),
        bytes: scan_path(path)?.bytes,
        reason: reason.to_string(),
        validation_facts,
        validation_dir,
    })
}

fn skip(session_id: &str, path: &Path, reason: &str) -> StorageMaintenanceSkip {
    StorageMaintenanceSkip {
        session_id: Some(session_id.to_string()),
        path: path_string(path),
        reason: reason.to_string(),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
