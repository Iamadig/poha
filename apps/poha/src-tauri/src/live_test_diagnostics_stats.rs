use std::path::Path;

use poha_audio_utils::Source;
use serde::Serialize;
use serde_json::Value;

const MIN_FINAL_DURATION_MS: u64 = 5_000;
const MIN_STEM_DURATION_MS: u64 = 5_000;
const MIN_AUDIBLE_RMS_DBFS: f64 = -60.0;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AudioAnalysisWindow {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AudioStats {
    path: String,
    exists: bool,
    bytes: Option<u64>,
    channels: Option<u16>,
    sample_rate: Option<u32>,
    duration_ms: Option<u64>,
    analyzed_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis_window: Option<AudioAnalysisWindow>,
    rms_dbfs: Option<f64>,
    channel_rms_dbfs: Vec<Option<f64>>,
    error: Option<String>,
}

impl AudioStats {
    pub(super) fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    pub(super) fn rms_dbfs(&self) -> Option<f64> {
        self.rms_dbfs
    }

    pub(super) fn channel_rms_dbfs(&self, index: usize) -> Option<f64> {
        self.channel_rms_dbfs.get(index).copied().flatten()
    }

    pub(super) fn mixed_mp3_passed(&self) -> bool {
        self.exists
            && self.channels == Some(2)
            && self
                .duration_ms
                .is_some_and(|ms| ms >= MIN_FINAL_DURATION_MS)
            && self.error.is_none()
    }

    pub(super) fn stem_passed(&self) -> bool {
        self.exists
            && self.error.is_none()
            && self
                .duration_ms
                .is_some_and(|ms| ms >= MIN_STEM_DURATION_MS)
            && self.rms_dbfs.is_some_and(|rms| rms >= MIN_AUDIBLE_RMS_DBFS)
    }

    pub(super) fn audible_window_passed(&self, min_ms: u64, min_rms_dbfs: f64) -> bool {
        self.exists
            && self.error.is_none()
            && self.analyzed_duration_ms.is_some_and(|ms| ms >= min_ms)
            && self.rms_dbfs.is_some_and(|rms| rms >= min_rms_dbfs)
    }

    pub(super) fn channel_audible_window_passed(
        &self,
        index: usize,
        min_ms: u64,
        min_rms_dbfs: f64,
    ) -> bool {
        self.exists
            && self.error.is_none()
            && self.analyzed_duration_ms.is_some_and(|ms| ms >= min_ms)
            && self
                .channel_rms_dbfs(index)
                .is_some_and(|rms| rms >= min_rms_dbfs)
    }

    fn playable(&self) -> bool {
        self.exists && self.error.is_none()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TranscriptStats {
    path: String,
    exists: bool,
    bytes: Option<u64>,
    segment_count: usize,
    contains_diagnostic_phrase: bool,
    text_preview: String,
    #[serde(skip)]
    text_normalized: String,
    #[serde(skip)]
    segments: Vec<TranscriptSegmentStat>,
    error: Option<String>,
}

impl TranscriptStats {
    pub(super) fn has_segments(&self) -> bool {
        self.segment_count > 0
    }

    pub(super) fn contains_phrase(&self, phrase: &str) -> bool {
        !phrase.trim().is_empty() && self.text_normalized.contains(&normalize_text(phrase))
    }

    pub(super) fn phrase_start_ms(&self, phrase: &str) -> Option<i64> {
        let phrase = normalize_text(phrase);
        if phrase.is_empty() {
            return None;
        }
        self.segments
            .iter()
            .find(|segment| segment.text_normalized.contains(&phrase))
            .and_then(|segment| segment.start_ms)
    }
}

#[derive(Debug, Clone)]
struct TranscriptSegmentStat {
    start_ms: Option<i64>,
    text_normalized: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageLifecycleStats {
    mixed_mp3_present: bool,
    legacy_mixed_wav_present: bool,
    mic_stem_present: bool,
    processed_mic_stem_present: bool,
    system_stem_present: bool,
    transcript_chunks_present: bool,
    stem_state: String,
    capture_scratch_audio_present: bool,
    capture_scratch_audio_file_count: usize,
    capture_scratch_state: String,
    expectation: String,
}

impl StorageLifecycleStats {
    pub(super) fn passed(&self) -> bool {
        self.mixed_mp3_present
            && !self.legacy_mixed_wav_present
            && matches!(
                self.stem_state.as_str(),
                "cleanupCandidate" | "alreadyCleaned"
            )
    }
}

pub(super) fn audio_stats(path: &Path) -> AudioStats {
    audio_stats_for_window(path, None)
}

pub(super) fn audio_stats_for_window(
    path: &Path,
    analysis_window: Option<AudioAnalysisWindow>,
) -> AudioStats {
    let metadata = std::fs::metadata(path).ok();
    let mut stats = AudioStats {
        path: path_string(path),
        exists: metadata.is_some(),
        bytes: metadata.map(|metadata| metadata.len()),
        channels: None,
        sample_rate: None,
        duration_ms: None,
        analyzed_duration_ms: None,
        analysis_window,
        rms_dbfs: None,
        channel_rms_dbfs: Vec::new(),
        error: None,
    };
    if !stats.exists {
        stats.error = Some("file missing".to_string());
        return stats;
    }

    match audio_level(path, analysis_window) {
        Ok(level) => {
            stats.channels = Some(level.channels);
            stats.sample_rate = Some(level.sample_rate);
            stats.duration_ms = level.duration_ms;
            stats.analyzed_duration_ms = level.analyzed_duration_ms;
            stats.rms_dbfs = level.rms_dbfs;
            stats.channel_rms_dbfs = level.channel_rms_dbfs;
        }
        Err(error) => stats.error = Some(error),
    }
    stats
}

pub(super) fn transcript_stats(path: &Path) -> TranscriptStats {
    let metadata = std::fs::metadata(path).ok();
    let mut stats = TranscriptStats {
        path: path_string(path),
        exists: metadata.is_some(),
        bytes: metadata.map(|metadata| metadata.len()),
        segment_count: 0,
        contains_diagnostic_phrase: false,
        text_preview: String::new(),
        text_normalized: String::new(),
        segments: Vec::new(),
        error: None,
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        stats.error = Some("file missing".to_string());
        return stats;
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        stats.error = Some("failed parsing transcript JSON".to_string());
        return stats;
    };
    let Some(segments) = json.get("segments").and_then(Value::as_array) else {
        stats.error = Some("transcript JSON has no segments array".to_string());
        return stats;
    };
    let segment_stats = segments
        .iter()
        .filter_map(|segment| {
            let text = segment.get("text").and_then(Value::as_str)?;
            Some(TranscriptSegmentStat {
                start_ms: segment
                    .get("startMs")
                    .or_else(|| segment.get("start_ms"))
                    .and_then(Value::as_i64),
                text_normalized: normalize_text(text),
            })
        })
        .collect::<Vec<_>>();
    let text = segments
        .iter()
        .filter_map(|segment| segment.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    stats.segment_count = segments.len();
    stats.contains_diagnostic_phrase = text.to_lowercase().contains("poha live test");
    stats.text_preview = text.chars().take(240).collect();
    stats.text_normalized = normalize_text(&text);
    stats.segments = segment_stats;
    stats
}

pub(super) fn storage_lifecycle_stats(
    output_dir: &Path,
    capture_dir: &Path,
    mixed_stats: &AudioStats,
) -> StorageLifecycleStats {
    let mic_stem_present = output_dir.join("audio_mic.wav").exists();
    let processed_mic_stem_present = output_dir.join("audio_mic_processed.wav").exists();
    let system_stem_present = output_dir.join("audio_spk.wav").exists();
    let stem_state = match (
        mic_stem_present,
        processed_mic_stem_present,
        system_stem_present,
    ) {
        (true, _, true) => "cleanupCandidate",
        (false, false, false) => "alreadyCleaned",
        _ => "partialStemSet",
    };
    let capture_scratch_audio_file_count =
        capture_scratch_audio_file_count(output_dir, capture_dir);
    let capture_scratch_state = if same_path(output_dir, capture_dir) {
        "notSeparate"
    } else if capture_scratch_audio_file_count == 0 {
        "alreadyCleaned"
    } else {
        "cleanupCandidate"
    };

    StorageLifecycleStats {
        mixed_mp3_present: mixed_stats.playable(),
        legacy_mixed_wav_present: output_dir.join("audio.wav").exists(),
        mic_stem_present,
        processed_mic_stem_present,
        system_stem_present,
        transcript_chunks_present: output_dir.join("transcript_chunks").exists(),
        stem_state: stem_state.to_string(),
        capture_scratch_audio_present: capture_scratch_audio_file_count > 0,
        capture_scratch_audio_file_count,
        capture_scratch_state: capture_scratch_state.to_string(),
        expectation: "Keep mixed audio.mp3 as playable source; do not leave legacy audio.wav; output stem WAVs and capture scratch audio should be cleanup candidates or already cleaned.".to_string(),
    }
}

fn capture_scratch_audio_file_count(output_dir: &Path, capture_dir: &Path) -> usize {
    if same_path(output_dir, capture_dir) {
        return 0;
    }
    [
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        "audio.m4a",
        "audio_mic.wav",
        "audio_mic_processed.wav",
        "audio_spk.wav",
    ]
    .iter()
    .filter(|name| capture_dir.join(name).exists())
    .count()
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

struct AudioLevel {
    channels: u16,
    sample_rate: u32,
    duration_ms: Option<u64>,
    analyzed_duration_ms: Option<u64>,
    rms_dbfs: Option<f64>,
    channel_rms_dbfs: Vec<Option<f64>>,
}

fn audio_level(
    path: &Path,
    analysis_window: Option<AudioAnalysisWindow>,
) -> Result<AudioLevel, String> {
    let source = poha_audio_utils::source_from_path(path)
        .map_err(|error| format!("failed decoding audio: {error}"))?;
    let channels = u16::from(source.channels());
    let sample_rate = u32::from(source.sample_rate());
    let duration_ms = source
        .total_duration()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .filter(|ms| *ms > 0);
    let channel_count = usize::from(channels.max(1));
    let start_frame = analysis_window
        .map(|window| ms_to_frames(window.start_ms, sample_rate))
        .unwrap_or(0);
    let end_frame = analysis_window.map(|window| ms_to_frames(window.end_ms, sample_rate));
    let mut channel_sums = vec![0.0f64; channel_count];
    let mut channel_counts = vec![0u64; channel_count];
    let mut sum = 0.0f64;
    let mut count = 0u64;
    let mut source_index = 0u64;

    for sample in source {
        let frame = source_index / u64::from(channels.max(1));
        let channel = usize::try_from(source_index % u64::from(channels.max(1))).unwrap_or(0);
        source_index += 1;
        if frame < start_frame {
            continue;
        }
        if end_frame.is_some_and(|end_frame| frame >= end_frame) {
            break;
        }
        let sample = f64::from(sample);
        if !sample.is_finite() {
            continue;
        }
        let square = sample * sample;
        channel_sums[channel] += square;
        channel_counts[channel] += 1;
        sum += square;
        count += 1;
    }

    let analyzed_duration_ms = if channels == 0 || sample_rate == 0 {
        None
    } else {
        count
            .checked_div(u64::from(channels))
            .and_then(|frames| frames.checked_mul(1_000))
            .and_then(|ms| ms.checked_div(u64::from(sample_rate)))
            .filter(|ms| *ms > 0)
    };

    Ok(AudioLevel {
        channels,
        sample_rate,
        duration_ms: duration_ms.or(analyzed_duration_ms),
        analyzed_duration_ms,
        rms_dbfs: rms_dbfs(sum, count).map(round_db),
        channel_rms_dbfs: channel_sums
            .into_iter()
            .zip(channel_counts)
            .map(|(sum, count)| rms_dbfs(sum, count).map(round_db))
            .collect(),
    })
}

fn ms_to_frames(ms: u64, sample_rate: u32) -> u64 {
    ms.saturating_mul(u64::from(sample_rate)) / 1_000
}

fn rms_dbfs(sum_squares: f64, count: u64) -> Option<f64> {
    if count == 0 {
        return None;
    }
    let rms = (sum_squares / count as f64).sqrt();
    (rms > 0.0).then(|| 20.0 * rms.log10())
}

fn round_db(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn normalize_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
