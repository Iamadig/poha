use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

use poha_audio_utils::Source;

use super::audio_quality::{
    StorageMixedAudioQuality, enforce_mixed_audio_quality, mixed_audio_quality,
};
use super::scan::{metadata_string, path_string, read_json, valid_audio_file};

const SOURCE_AUDIO_FILES: &[&str] = &[
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
    "audio.m4a",
    "audio_mic.wav",
    "audio_mic_processed.wav",
    "audio_spk.wav",
];
const MIN_SOURCE_TOLERANCE_MS: u64 = 5_000;
const MIN_SESSION_TOLERANCE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageCandidateKind {
    LegacyWav,
    LegacyWavTranscode,
    StemWav,
    CaptureScratchAudio,
    TranscriptChunks,
}

impl StorageCandidateKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::LegacyWav => "legacyWav",
            Self::LegacyWavTranscode => "legacyWavTranscode",
            Self::StemWav => "stemWav",
            Self::CaptureScratchAudio => "captureScratchAudio",
            Self::TranscriptChunks => "transcriptChunks",
        }
    }

    pub(super) fn is_source_audio(self) -> bool {
        matches!(
            self,
            Self::LegacyWav | Self::LegacyWavTranscode | Self::StemWav | Self::CaptureScratchAudio
        )
    }

    fn transcodes_legacy_wav(self) -> bool {
        self == Self::LegacyWavTranscode
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageValidationFacts {
    session_status: Option<String>,
    transcript_markdown_bytes: u64,
    transcript_segment_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunk_manifest_state: Option<String>,
    mixed_audio_path: String,
    mixed_audio_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mixed_audio_duration_ms: Option<u64>,
    transcodes_legacy_wav: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_audio_duration_ms: Option<u64>,
    duration_checks: Vec<StorageDurationCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mixed_audio_quality: Option<StorageMixedAudioQuality>,
    playable_source_audio_count_after_move: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StorageDurationCheck {
    source: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reference_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tolerance_ms: Option<u64>,
}

pub(super) struct TranscriptValidation {
    markdown_bytes: u64,
    segment_count: usize,
}

pub(super) struct ChunkManifestValidation {
    pub(super) is_complete: bool,
    pub(super) state: String,
}

pub(super) fn transcript_validation(session_dir: &Path) -> Option<TranscriptValidation> {
    let markdown_bytes = fs::metadata(session_dir.join("transcript.md")).ok()?.len();
    if markdown_bytes == 0 {
        return None;
    }
    let json = read_json(&session_dir.join("transcript.json")).ok()?;
    let segments = json.get("segments")?.as_array()?;
    Some(TranscriptValidation {
        markdown_bytes,
        segment_count: segments.len(),
    })
}

pub(super) fn chunk_manifest_validation(session_dir: &Path) -> ChunkManifestValidation {
    let paths = ["transcript.chunks.json", "transcript.live.json"]
        .iter()
        .map(|name| session_dir.join(name))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return ChunkManifestValidation {
            is_complete: true,
            state: "absentWithFinalTranscript".to_string(),
        };
    }
    for path in paths {
        let Ok(value) = read_json(&path) else {
            return ChunkManifestValidation {
                is_complete: false,
                state: format!("chunk manifest is invalid: {}", path_string(&path)),
            };
        };
        if manifest_has_unfinished_chunks(&value) {
            return ChunkManifestValidation {
                is_complete: false,
                state: format!(
                    "chunk manifest has unfinished chunks: {}",
                    path_string(&path)
                ),
            };
        }
    }
    ChunkManifestValidation {
        is_complete: true,
        state: "complete".to_string(),
    }
}

pub(super) fn validation_facts(
    session_dir: &Path,
    metadata: Option<&Value>,
    transcript: &TranscriptValidation,
    candidate_path: Option<&Path>,
    kind: StorageCandidateKind,
    chunk_manifest_state: Option<String>,
    mixed_mp3_planned: bool,
) -> Result<StorageValidationFacts, String> {
    let mixed_audio_path = session_dir.join("audio.mp3");
    let mixed_audio_valid = valid_audio_file(&mixed_audio_path);
    let mixed_audio_duration_ms = mixed_audio_valid
        .then(|| audio_duration_ms(&mixed_audio_path))
        .flatten();
    let source_audio_duration_ms = candidate_path.and_then(audio_duration_ms);
    let mut duration_checks = Vec::new();

    if kind.is_source_audio() && source_audio_duration_ms.is_none() {
        return Err("source audio duration is unavailable".to_string());
    }
    let pending_mixed_mp3 = !mixed_audio_valid && mixed_mp3_planned;
    if kind.is_source_audio() && !mixed_audio_valid && !pending_mixed_mp3 {
        return Err("valid mixed MP3 is missing".to_string());
    }
    if kind.is_source_audio() && mixed_audio_valid && mixed_audio_duration_ms.is_none() {
        return Err("mixed MP3 duration is unavailable".to_string());
    }

    if let Some(reference_ms) = source_audio_duration_ms {
        add_duration_check(
            &mut duration_checks,
            "sourceAudio",
            mixed_audio_duration_ms,
            Some(reference_ms),
            source_tolerance_ms(reference_ms),
            true,
        )?;
    }
    if let Some(reference_ms) = metadata.and_then(session_duration_ms) {
        add_duration_check(
            &mut duration_checks,
            "session",
            mixed_audio_duration_ms,
            Some(reference_ms),
            session_tolerance_ms(reference_ms),
            false,
        )?;
    }

    let playable_source_audio_count_after_move = playable_source_audio_count_after_move(
        session_dir,
        candidate_path,
        mixed_audio_valid || mixed_mp3_planned,
    );
    if kind.is_source_audio() && playable_source_audio_count_after_move == 0 {
        return Err("would leave session without playable source audio".to_string());
    }
    let mixed_audio_quality = if requires_mixed_audio_quality(kind, candidate_path) {
        let quality = mixed_audio_quality(&mixed_audio_path)?;
        enforce_mixed_audio_quality(&quality)?;
        Some(quality)
    } else {
        None
    };

    Ok(StorageValidationFacts {
        session_status: metadata.and_then(|value| metadata_string(value, "status")),
        transcript_markdown_bytes: transcript.markdown_bytes,
        transcript_segment_count: transcript.segment_count,
        chunk_manifest_state,
        mixed_audio_path: path_string(&mixed_audio_path),
        mixed_audio_valid,
        mixed_audio_duration_ms,
        transcodes_legacy_wav: kind.transcodes_legacy_wav(),
        source_audio_duration_ms,
        duration_checks,
        mixed_audio_quality,
        playable_source_audio_count_after_move,
    })
}

fn requires_mixed_audio_quality(kind: StorageCandidateKind, candidate_path: Option<&Path>) -> bool {
    kind == StorageCandidateKind::StemWav
        || (kind == StorageCandidateKind::CaptureScratchAudio
            && candidate_path.is_some_and(is_stem_wav_path))
}

fn manifest_has_unfinished_chunks(value: &Value) -> bool {
    let Some(chunks) = value.get("chunks").and_then(Value::as_array) else {
        return false;
    };
    chunks.iter().any(|chunk| {
        chunk
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "done")
    })
}

fn add_duration_check(
    checks: &mut Vec<StorageDurationCheck>,
    source: &str,
    actual_ms: Option<u64>,
    reference_ms: Option<u64>,
    tolerance_ms: u64,
    enforce: bool,
) -> Result<(), String> {
    let Some(reference_ms) = reference_ms else {
        checks.push(check(source, "noReference", actual_ms, None, None));
        return Ok(());
    };
    let Some(actual_ms) = actual_ms else {
        checks.push(check(
            source,
            "pendingAfterTranscode",
            None,
            Some(reference_ms),
            Some(tolerance_ms),
        ));
        return Ok(());
    };
    let delta = actual_ms.abs_diff(reference_ms);
    if delta > tolerance_ms {
        if !enforce {
            checks.push(check(
                source,
                "outsideToleranceAuditOnly",
                Some(actual_ms),
                Some(reference_ms),
                Some(tolerance_ms),
            ));
            return Ok(());
        }
        return Err(format!(
            "mixed MP3 duration {actual_ms}ms differs from {source} {reference_ms}ms by more than {tolerance_ms}ms"
        ));
    }
    checks.push(check(
        source,
        "passed",
        Some(actual_ms),
        Some(reference_ms),
        Some(tolerance_ms),
    ));
    Ok(())
}

fn check(
    source: &str,
    status: &str,
    actual_ms: Option<u64>,
    reference_ms: Option<u64>,
    tolerance_ms: Option<u64>,
) -> StorageDurationCheck {
    StorageDurationCheck {
        source: source.to_string(),
        status: status.to_string(),
        actual_ms,
        reference_ms,
        tolerance_ms,
    }
}

fn audio_duration_ms(path: &Path) -> Option<u64> {
    let source = poha_audio_utils::source_from_path(path).ok()?;
    if let Some(duration) = source.total_duration() {
        return u64::try_from(duration.as_millis())
            .ok()
            .filter(|ms| *ms > 0);
    }
    let channels = u64::from(u16::from(source.channels()));
    let sample_rate = u64::from(u32::from(source.sample_rate()));
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let samples = u64::try_from(source.count()).ok()?;
    samples
        .checked_div(channels)?
        .checked_mul(1_000)?
        .checked_div(sample_rate)
        .filter(|ms| *ms > 0)
}

fn session_duration_ms(metadata: &Value) -> Option<u64> {
    let started_at = metadata_string(metadata, "started_at")?;
    let ended_at = metadata_string(metadata, "ended_at")?;
    let started_at = DateTime::parse_from_rfc3339(&started_at)
        .ok()?
        .with_timezone(&Utc);
    let ended_at = DateTime::parse_from_rfc3339(&ended_at)
        .ok()?
        .with_timezone(&Utc);
    let duration = ended_at.signed_duration_since(started_at).to_std().ok()?;
    u64::try_from(duration.as_millis())
        .ok()
        .filter(|ms| *ms > 0)
}

fn source_tolerance_ms(reference_ms: u64) -> u64 {
    MIN_SOURCE_TOLERANCE_MS.max(reference_ms / 100)
}

fn session_tolerance_ms(reference_ms: u64) -> u64 {
    MIN_SESSION_TOLERANCE_MS.max(reference_ms / 10)
}

fn playable_source_audio_count_after_move(
    session_dir: &Path,
    moving_path: Option<&Path>,
    mixed_mp3_planned: bool,
) -> usize {
    SOURCE_AUDIO_FILES
        .iter()
        .filter(|file_name| {
            let path = session_dir.join(file_name);
            if moving_path.is_some_and(|moving| moving == path.as_path()) {
                return false;
            }
            if **file_name == "audio.mp3" && mixed_mp3_planned {
                return true;
            }
            valid_audio_file(&path)
        })
        .count()
}

fn is_stem_wav_path(path: &Path) -> bool {
    ["audio_mic.wav", "audio_mic_processed.wav", "audio_spk.wav"]
        .iter()
        .any(|file_name| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
        })
}
