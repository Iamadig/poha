use chrono::Utc;
use serde::Serialize;
use std::path::Path;

use super::StorageError;
use super::audio_quality::{
    AUDIBLE_SYSTEM_RMS_DBFS, MAX_MIC_STEM_DROP_DB, MIN_MIC_RMS_DBFS, StorageAudioQualityThresholds,
    StorageMixedAudioQuality, StorageStemAudioQuality, audio_quality_thresholds,
    mixed_audio_quality_with_limit, stem_audio_quality_with_limit,
};
use super::scan::{
    collect_session_dirs, ensure_recordings_dir, file_name_string, metadata_string, path_string,
    read_json,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageAudioQualityReport {
    recordings_dir: String,
    generated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis_window_seconds: Option<u64>,
    thresholds: StorageAudioQualityThresholds,
    totals: StorageAudioQualityTotals,
    sessions: Vec<StorageAudioQualitySession>,
}

pub const DEFAULT_AUDIO_QUALITY_ANALYSIS_WINDOW_SECONDS: u64 = 30;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageAudioQualityTotals {
    scanned_session_count: usize,
    affected_session_count: usize,
    ok_session_count: usize,
    review_session_count: usize,
    regenerate_mp3_session_count: usize,
    skipped_active_session_count: usize,
    skipped_no_audio_session_count: usize,
    omitted_ok_session_count: usize,
    returned_session_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageAudioQualitySession {
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<String>,
    status: String,
    recommendation: String,
    reason: String,
    can_regenerate_mp3: bool,
    repairable_by_regeneration: bool,
    audio_mp3_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_mic_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_spk_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    legacy_wav_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mixed_audio_quality: Option<StorageMixedAudioQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mic_stem_quality: Option<StorageStemAudioQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_stem_quality: Option<StorageStemAudioQuality>,
}

pub fn audio_quality_report(
    recordings_dir: &Path,
    limit: usize,
    include_ok: bool,
) -> Result<StorageAudioQualityReport, StorageError> {
    audio_quality_report_with_options(
        recordings_dir,
        limit,
        include_ok,
        Some(DEFAULT_AUDIO_QUALITY_ANALYSIS_WINDOW_SECONDS),
    )
}

pub fn audio_quality_report_with_options(
    recordings_dir: &Path,
    limit: usize,
    include_ok: bool,
    analysis_window_seconds: Option<u64>,
) -> Result<StorageAudioQualityReport, StorageError> {
    ensure_recordings_dir(recordings_dir)?;

    let mut totals = StorageAudioQualityTotals::default();
    let mut sessions = Vec::new();
    for session_dir in collect_session_dirs(recordings_dir)? {
        let metadata = read_json(&session_dir.join("session.json")).ok();
        let status = metadata
            .as_ref()
            .and_then(|value| metadata_string(value, "status"))
            .unwrap_or_default();
        if is_active_status(&status) {
            totals.skipped_active_session_count += 1;
            continue;
        }
        if !session_has_audio(&session_dir) {
            totals.skipped_no_audio_session_count += 1;
            continue;
        }
        totals.scanned_session_count += 1;

        let session = analyze_session(&session_dir, metadata.as_ref(), analysis_window_seconds)?;
        match session.recommendation.as_str() {
            "ok" => {
                totals.ok_session_count += 1;
                if include_ok {
                    sessions.push(session);
                } else {
                    totals.omitted_ok_session_count += 1;
                }
            }
            "regenerateMp3" => {
                totals.affected_session_count += 1;
                totals.regenerate_mp3_session_count += 1;
                sessions.push(session);
            }
            _ => {
                totals.affected_session_count += 1;
                totals.review_session_count += 1;
                sessions.push(session);
            }
        }
    }

    sessions.sort_by(|left, right| {
        recommendation_rank(&left.recommendation)
            .cmp(&recommendation_rank(&right.recommendation))
            .then_with(|| status_rank(&left.status).cmp(&status_rank(&right.status)))
            .then_with(|| right.started_at.cmp(&left.started_at))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    sessions.truncate(limit);
    totals.returned_session_count = sessions.len();

    Ok(StorageAudioQualityReport {
        recordings_dir: path_string(recordings_dir),
        generated_at: Utc::now().to_rfc3339(),
        analysis_window_seconds,
        thresholds: audio_quality_thresholds(),
        totals,
        sessions,
    })
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "recording" | "queued" | "transcribing")
}

fn session_has_audio(session_dir: &Path) -> bool {
    [
        "audio.mp3",
        "audio.wav",
        "audio_mic.wav",
        "audio_mic_processed.wav",
        "audio_spk.wav",
    ]
    .iter()
    .any(|file_name| session_dir.join(file_name).exists())
}

fn analyze_session(
    session_dir: &Path,
    metadata: Option<&serde_json::Value>,
    analysis_window_seconds: Option<u64>,
) -> Result<StorageAudioQualitySession, StorageError> {
    let session_id = metadata
        .and_then(|value| metadata_string(value, "id"))
        .unwrap_or_else(|| file_name_string(session_dir));
    let meeting = read_json(&session_dir.join("meeting.json")).ok();
    let title = meeting
        .as_ref()
        .and_then(|value| metadata_string(value, "title"))
        .or_else(|| metadata.and_then(|value| metadata_string(value, "title")));
    let started_at = metadata.and_then(|value| metadata_string(value, "started_at"));
    let ended_at = metadata.and_then(|value| metadata_string(value, "ended_at"));

    let mp3_path = session_dir.join("audio.mp3");
    let mic_stem_path = session_dir.join("audio_mic.wav");
    let system_stem_path = session_dir.join("audio_spk.wav");
    let legacy_wav_path = session_dir.join("audio.wav");
    let has_mic_stem = mic_stem_path.exists();
    let has_system_stem = system_stem_path.exists();
    let has_legacy_wav = legacy_wav_path.exists();
    let can_regenerate_mp3 = has_legacy_wav || (has_mic_stem && has_system_stem);

    let mic_stem_quality = if has_mic_stem {
        stem_audio_quality_with_limit(&mic_stem_path, analysis_window_seconds).ok()
    } else {
        None
    };
    let system_stem_quality = if has_system_stem {
        stem_audio_quality_with_limit(&system_stem_path, analysis_window_seconds).ok()
    } else {
        None
    };

    let (status, reason, mixed_audio_quality) = if !mp3_path.exists() {
        (
            "missingMp3".to_string(),
            "mixed audio.mp3 is missing".to_string(),
            None,
        )
    } else {
        match mixed_audio_quality_with_limit(&mp3_path, analysis_window_seconds) {
            Ok(quality) => {
                let status = mp3_status_with_stem_comparison(&quality, mic_stem_quality.as_ref());
                let reason = reason_for_status(&status);
                (status, reason, Some(quality))
            }
            Err(error) => (
                "invalidMp3".to_string(),
                format!("mixed audio.mp3 cannot be decoded: {error}"),
                None,
            ),
        }
    };
    let repairable_by_regeneration = regeneration_likely_to_help(
        &status,
        can_regenerate_mp3,
        has_legacy_wav,
        mixed_audio_quality.as_ref(),
        mic_stem_quality.as_ref(),
        system_stem_quality.as_ref(),
    );
    let recommendation = recommendation_for_status(&status, repairable_by_regeneration);

    Ok(StorageAudioQualitySession {
        session_id,
        title,
        started_at,
        ended_at,
        status,
        recommendation,
        reason,
        can_regenerate_mp3,
        repairable_by_regeneration,
        audio_mp3_path: path_string(&mp3_path),
        audio_mic_path: has_mic_stem.then(|| path_string(&mic_stem_path)),
        audio_spk_path: has_system_stem.then(|| path_string(&system_stem_path)),
        legacy_wav_path: has_legacy_wav.then(|| path_string(&legacy_wav_path)),
        mixed_audio_quality,
        mic_stem_quality,
        system_stem_quality,
    })
}

fn mp3_status_with_stem_comparison(
    mixed: &StorageMixedAudioQuality,
    mic_stem: Option<&StorageStemAudioQuality>,
) -> String {
    if mixed.status != "passed" {
        return mixed.status.clone();
    }
    let Some(stem_rms_dbfs) = mic_stem.and_then(StorageStemAudioQuality::rms_dbfs) else {
        return "ok".to_string();
    };
    let Some(mixed_mic_rms_dbfs) = mixed.mic_rms_dbfs() else {
        return "ok".to_string();
    };
    let mic_drop_db = stem_rms_dbfs - mixed_mic_rms_dbfs;
    if stem_rms_dbfs >= MIN_MIC_RMS_DBFS && mic_drop_db > MAX_MIC_STEM_DROP_DB {
        "micDegradedFromStem".to_string()
    } else {
        "ok".to_string()
    }
}

fn regeneration_likely_to_help(
    status: &str,
    can_regenerate_mp3: bool,
    has_legacy_wav: bool,
    mixed: Option<&StorageMixedAudioQuality>,
    mic_stem: Option<&StorageStemAudioQuality>,
    system_stem: Option<&StorageStemAudioQuality>,
) -> bool {
    if !can_regenerate_mp3 || status == "ok" {
        return false;
    }
    if matches!(
        status,
        "missingMp3" | "invalidMp3" | "notStereo" | "micDegradedFromStem"
    ) {
        return true;
    }
    if has_legacy_wav && mic_stem.is_none() {
        return true;
    }

    match status {
        "micSilent" | "micTooQuiet" => mic_stem
            .and_then(StorageStemAudioQuality::rms_dbfs)
            .is_some_and(|stem_rms_dbfs| stem_rms_dbfs >= MIN_MIC_RMS_DBFS),
        "micTooQuietRelativeToSystem" => {
            let Some(mic_stem_rms_dbfs) = mic_stem.and_then(StorageStemAudioQuality::rms_dbfs)
            else {
                return has_legacy_wav;
            };
            let Some(system_stem_rms_dbfs) =
                system_stem.and_then(StorageStemAudioQuality::rms_dbfs)
            else {
                return mic_stem_rms_dbfs >= MIN_MIC_RMS_DBFS;
            };
            if system_stem_rms_dbfs <= AUDIBLE_SYSTEM_RMS_DBFS {
                return mic_stem_rms_dbfs >= MIN_MIC_RMS_DBFS;
            }
            let stem_delta_db = mic_stem_rms_dbfs - system_stem_rms_dbfs;
            stem_delta_db >= -MAX_MIC_STEM_DROP_DB
                || mixed
                    .and_then(StorageMixedAudioQuality::mic_rms_dbfs)
                    .is_some_and(|mixed_mic_rms_dbfs| {
                        mic_stem_rms_dbfs - mixed_mic_rms_dbfs > MAX_MIC_STEM_DROP_DB
                    })
        }
        _ => false,
    }
}

fn recommendation_for_status(status: &str, repairable_by_regeneration: bool) -> String {
    if status == "ok" {
        "ok"
    } else if repairable_by_regeneration {
        "regenerateMp3"
    } else {
        "review"
    }
    .to_string()
}

fn reason_for_status(status: &str) -> String {
    match status {
        "ok" => "mixed MP3 mic/system audio quality passed".to_string(),
        "notStereo" => "mixed audio.mp3 is not stereo".to_string(),
        "micSilent" => "mixed audio.mp3 mic channel is silent".to_string(),
        "micTooQuiet" => "mixed audio.mp3 mic channel is below loudness threshold".to_string(),
        "micTooQuietRelativeToSystem" => {
            "mixed audio.mp3 mic channel is too quiet relative to system channel".to_string()
        }
        "micDegradedFromStem" => {
            "mixed audio.mp3 mic channel is much quieter than audio_mic.wav".to_string()
        }
        _ => format!("mixed audio.mp3 audio quality check failed: {status}"),
    }
}

fn recommendation_rank(recommendation: &str) -> u8 {
    match recommendation {
        "regenerateMp3" => 0,
        "review" => 1,
        _ => 2,
    }
}

fn status_rank(status: &str) -> u8 {
    match status {
        "missingMp3" | "invalidMp3" => 0,
        "micSilent" | "micTooQuiet" | "micTooQuietRelativeToSystem" => 1,
        "micDegradedFromStem" => 2,
        "notStereo" => 3,
        _ => 4,
    }
}
