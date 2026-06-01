use serde::Serialize;
use std::path::Path;

use super::StorageError;
use super::scan::{
    AudioFormatSummary, DELETED_DIR, ScanSummary, audio_formats, collect_session_dirs,
    file_name_string, metadata_string, path_string, read_json, scan_path,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    recordings_dir: String,
    totals: StorageTotals,
    audio_by_format: Vec<AudioFormatSummary>,
    largest_sessions: Vec<StorageSessionSummary>,
    reclaimable: Vec<ReclaimableCandidate>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageTotals {
    total_bytes: u64,
    active_recordings_bytes: u64,
    active_recordings_count: usize,
    deleted_recordings_bytes: u64,
    deleted_recordings_count: usize,
    poha_metadata_bytes: u64,
    other_bytes: u64,
    audio_bytes: u64,
    audio_file_count: u64,
    source_audio_bytes: u64,
    source_audio_file_count: u64,
    mixed_source_audio_bytes: u64,
    mixed_source_audio_file_count: u64,
    legacy_wav_source_audio_bytes: u64,
    legacy_wav_source_audio_file_count: u64,
    stem_wav_bytes: u64,
    stem_wav_file_count: u64,
    derived_chunk_audio_bytes: u64,
    derived_chunk_audio_file_count: u64,
    other_audio_bytes: u64,
    other_audio_file_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSessionSummary {
    id: String,
    state: StorageSessionState,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    bytes: u64,
    audio_bytes: u64,
    audio_file_count: u64,
    source_audio_bytes: u64,
    source_audio_file_count: u64,
    mixed_source_audio_bytes: u64,
    legacy_wav_source_audio_bytes: u64,
    stem_wav_bytes: u64,
    derived_chunk_audio_bytes: u64,
    other_audio_bytes: u64,
    audio_by_format: Vec<AudioFormatSummary>,
    path: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum StorageSessionState {
    Active,
    Deleted,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReclaimableCandidate {
    kind: String,
    bytes: u64,
    count: usize,
    action: String,
    safety: String,
}

pub fn report(recordings_dir: &Path, limit: usize) -> Result<StorageReport, StorageError> {
    super::scan::ensure_recordings_dir(recordings_dir)?;

    let mut active = collect_sessions(recordings_dir, StorageSessionState::Active)?;
    let deleted_root = recordings_dir.join(DELETED_DIR);
    let mut deleted = if deleted_root.is_dir() {
        collect_sessions(&deleted_root, StorageSessionState::Deleted)?
    } else {
        Vec::new()
    };

    let total_scan = scan_path(recordings_dir)?;
    let poha_dir = recordings_dir.join(".poha");
    let poha_bytes = if poha_dir.is_dir() {
        scan_path(&poha_dir)?.bytes
    } else {
        0
    };
    let active_bytes = active.iter().map(|session| session.bytes).sum::<u64>();
    let deleted_bytes = deleted.iter().map(|session| session.bytes).sum::<u64>();
    let poha_metadata_bytes = poha_bytes.saturating_sub(deleted_bytes);
    let accounted_bytes = active_bytes
        .saturating_add(deleted_bytes)
        .saturating_add(poha_metadata_bytes);

    let mut largest_sessions = Vec::new();
    largest_sessions.append(&mut active);
    largest_sessions.append(&mut deleted);
    largest_sessions.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.id.cmp(&right.id))
    });
    largest_sessions.truncate(limit);

    let reclaimable =
        reclaimable_candidates(&total_scan, deleted_bytes, deleted_count(recordings_dir)?);

    Ok(StorageReport {
        recordings_dir: path_string(recordings_dir),
        totals: StorageTotals {
            total_bytes: total_scan.bytes,
            active_recordings_bytes: active_bytes,
            active_recordings_count: active_count(recordings_dir)?,
            deleted_recordings_bytes: deleted_bytes,
            deleted_recordings_count: deleted_count(recordings_dir)?,
            poha_metadata_bytes,
            other_bytes: total_scan.bytes.saturating_sub(accounted_bytes),
            audio_bytes: total_scan.audio_bytes,
            audio_file_count: total_scan.audio_file_count,
            source_audio_bytes: total_scan.source_audio_bytes,
            source_audio_file_count: total_scan.source_audio_file_count,
            mixed_source_audio_bytes: total_scan.mixed_source_audio_bytes,
            mixed_source_audio_file_count: total_scan.mixed_source_audio_file_count,
            legacy_wav_source_audio_bytes: total_scan.legacy_wav_source_audio_bytes,
            legacy_wav_source_audio_file_count: total_scan.legacy_wav_source_audio_file_count,
            stem_wav_bytes: total_scan.stem_wav_bytes,
            stem_wav_file_count: total_scan.stem_wav_file_count,
            derived_chunk_audio_bytes: total_scan.derived_chunk_audio_bytes,
            derived_chunk_audio_file_count: total_scan.derived_chunk_audio_file_count,
            other_audio_bytes: total_scan.other_audio_bytes,
            other_audio_file_count: total_scan.other_audio_file_count,
        },
        audio_by_format: audio_formats(total_scan.audio_by_format),
        largest_sessions,
        reclaimable,
    })
}

fn collect_sessions(
    root: &Path,
    state: StorageSessionState,
) -> Result<Vec<StorageSessionSummary>, StorageError> {
    let mut sessions = Vec::new();
    for path in collect_session_dirs(root)? {
        if path.file_name().is_some_and(|name| name == ".poha") {
            continue;
        }
        let scan = scan_path(&path)?;
        let metadata = read_json(&path.join("session.json")).ok();
        let id = metadata
            .as_ref()
            .and_then(|value| metadata_string(value, "id"))
            .unwrap_or_else(|| file_name_string(&path));
        sessions.push(StorageSessionSummary {
            id,
            state,
            status: metadata
                .as_ref()
                .and_then(|value| metadata_string(value, "status")),
            started_at: metadata
                .as_ref()
                .and_then(|value| metadata_string(value, "started_at")),
            ended_at: metadata
                .as_ref()
                .and_then(|value| metadata_string(value, "ended_at")),
            bytes: scan.bytes,
            audio_bytes: scan.audio_bytes,
            audio_file_count: scan.audio_file_count,
            source_audio_bytes: scan.source_audio_bytes,
            source_audio_file_count: scan.source_audio_file_count,
            mixed_source_audio_bytes: scan.mixed_source_audio_bytes,
            legacy_wav_source_audio_bytes: scan.legacy_wav_source_audio_bytes,
            stem_wav_bytes: scan.stem_wav_bytes,
            derived_chunk_audio_bytes: scan.derived_chunk_audio_bytes,
            other_audio_bytes: scan.other_audio_bytes,
            audio_by_format: audio_formats(scan.audio_by_format),
            path: path_string(&path),
        });
    }
    Ok(sessions)
}

fn reclaimable_candidates(
    scan: &ScanSummary,
    deleted_bytes: u64,
    deleted_count: usize,
) -> Vec<ReclaimableCandidate> {
    let mut reclaimable = Vec::new();
    if deleted_bytes > 0 {
        reclaimable.push(candidate(
            "deletedRecordings",
            deleted_bytes,
            deleted_count,
            "auto-trash soft-deleted recordings after retention",
            "soft-deleted recordings only; never hard-delete",
        ));
    }
    if scan.legacy_wav_source_audio_bytes > 0 {
        reclaimable.push(candidate(
            "legacyWavs",
            scan.legacy_wav_source_audio_bytes,
            scan.legacy_wav_source_audio_file_count as usize,
            "encode legacy mixed WAV to MP3, then move WAV to Trash",
            "keeps compressed mixed source audio before trashing WAV",
        ));
    }
    if scan.stem_wav_bytes > 0 {
        reclaimable.push(candidate(
            "stemWavs",
            scan.stem_wav_bytes,
            scan.stem_wav_file_count as usize,
            "move mic/system stem WAVs to Trash after transcript validation",
            "keeps mixed MP3, transcript, and metadata",
        ));
    }
    if scan.derived_chunk_audio_bytes > 0 {
        reclaimable.push(candidate(
            "transcriptionChunks",
            scan.derived_chunk_audio_bytes,
            scan.derived_chunk_audio_file_count as usize,
            "move transcript_chunks to Trash after transcript validation",
            "derived WAV chunks only",
        ));
    }
    reclaimable
}

fn candidate(
    kind: &str,
    bytes: u64,
    count: usize,
    action: &str,
    safety: &str,
) -> ReclaimableCandidate {
    ReclaimableCandidate {
        kind: kind.to_string(),
        bytes,
        count,
        action: action.to_string(),
        safety: safety.to_string(),
    }
}

fn active_count(recordings_dir: &Path) -> Result<usize, StorageError> {
    Ok(collect_session_dirs(recordings_dir)?.len())
}

fn deleted_count(recordings_dir: &Path) -> Result<usize, StorageError> {
    let deleted_root = recordings_dir.join(DELETED_DIR);
    if !deleted_root.is_dir() {
        return Ok(0);
    }
    Ok(collect_session_dirs(&deleted_root)?.len())
}
