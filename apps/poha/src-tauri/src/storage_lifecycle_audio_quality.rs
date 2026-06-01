use serde::Serialize;
use std::path::Path;

use poha_audio_utils::Source;

pub(super) const MIN_MIC_RMS_DBFS: f64 = -42.0;
pub(super) const AUDIBLE_SYSTEM_RMS_DBFS: f64 = -50.0;
pub(super) const MAX_MIC_BELOW_SYSTEM_DB: f64 = 18.0;
pub(super) const MAX_MIC_STEM_DROP_DB: f64 = 12.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageAudioQualityThresholds {
    min_mic_rms_dbfs: f64,
    audible_system_rms_dbfs: f64,
    max_mic_below_system_db: f64,
    max_mic_stem_drop_db: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageMixedAudioQuality {
    pub(super) status: String,
    channels: u16,
    sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analyzed_duration_ms: Option<u64>,
    mic_rms_dbfs: Option<f64>,
    system_rms_dbfs: Option<f64>,
    mic_minus_system_db: Option<f64>,
    min_mic_rms_dbfs: f64,
    audible_system_rms_dbfs: f64,
    max_mic_below_system_db: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageStemAudioQuality {
    path: String,
    channels: u16,
    sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    analyzed_duration_ms: Option<u64>,
    rms_dbfs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AudioLevelSummary {
    channels: u16,
    sample_rate: u32,
    duration_ms: Option<u64>,
    analyzed_duration_ms: Option<u64>,
    channel_rms_dbfs: Vec<Option<f64>>,
    overall_rms_dbfs: Option<f64>,
}

impl StorageMixedAudioQuality {
    pub(super) fn mic_rms_dbfs(&self) -> Option<f64> {
        self.mic_rms_dbfs
    }
}

impl StorageStemAudioQuality {
    pub(super) fn rms_dbfs(&self) -> Option<f64> {
        self.rms_dbfs
    }
}

pub(super) fn audio_quality_thresholds() -> StorageAudioQualityThresholds {
    StorageAudioQualityThresholds {
        min_mic_rms_dbfs: MIN_MIC_RMS_DBFS,
        audible_system_rms_dbfs: AUDIBLE_SYSTEM_RMS_DBFS,
        max_mic_below_system_db: MAX_MIC_BELOW_SYSTEM_DB,
        max_mic_stem_drop_db: MAX_MIC_STEM_DROP_DB,
    }
}

pub(super) fn mixed_audio_quality(path: &Path) -> Result<StorageMixedAudioQuality, String> {
    mixed_audio_quality_with_limit(path, None)
}

pub(super) fn mixed_audio_quality_with_limit(
    path: &Path,
    max_duration_secs: Option<u64>,
) -> Result<StorageMixedAudioQuality, String> {
    let level = audio_level(path, max_duration_secs)?;
    Ok(mixed_audio_quality_from_level(&level))
}

pub(super) fn stem_audio_quality_with_limit(
    path: &Path,
    max_duration_secs: Option<u64>,
) -> Result<StorageStemAudioQuality, String> {
    let level = audio_level(path, max_duration_secs)?;
    Ok(StorageStemAudioQuality {
        path: path_string(path),
        channels: level.channels,
        sample_rate: level.sample_rate,
        duration_ms: level.duration_ms,
        analyzed_duration_ms: level.analyzed_duration_ms,
        rms_dbfs: level.overall_rms_dbfs,
    })
}

pub(super) fn enforce_mixed_audio_quality(
    quality: &StorageMixedAudioQuality,
) -> Result<(), String> {
    if quality.status == "passed" {
        return Ok(());
    }
    Err(format!(
        "mixed MP3 audio quality check failed: {}",
        quality.status
    ))
}

fn audio_level(path: &Path, max_duration_secs: Option<u64>) -> Result<AudioLevelSummary, String> {
    let source = poha_audio_utils::source_from_path(path)
        .map_err(|error| format!("audio quality is unavailable: {error}"))?;
    let channels = u16::from(source.channels());
    let sample_rate = u32::from(source.sample_rate());
    let duration_ms = source
        .total_duration()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .filter(|ms| *ms > 0);
    let channel_count = usize::from(channels.max(1));
    let mut channel_sums = vec![0.0f64; channel_count];
    let mut channel_counts = vec![0u64; channel_count];
    let mut overall_sum = 0.0f64;
    let mut overall_count = 0u64;
    let max_samples = max_duration_secs
        .and_then(|seconds| seconds.checked_mul(u64::from(sample_rate)))
        .and_then(|frames| frames.checked_mul(u64::from(channels.max(1))));

    for sample in source {
        if max_samples.is_some_and(|max_samples| overall_count >= max_samples) {
            break;
        }
        let sample = f64::from(sample);
        let square = sample * sample;
        let channel = usize::try_from(overall_count)
            .ok()
            .map(|index| index % channel_count)
            .unwrap_or(0);
        channel_sums[channel] += square;
        channel_counts[channel] += 1;
        overall_sum += square;
        overall_count += 1;
    }

    let measured_duration_ms = duration_ms.or_else(|| {
        if channels == 0 || sample_rate == 0 {
            return None;
        }
        overall_count
            .checked_div(u64::from(channels))?
            .checked_mul(1_000)?
            .checked_div(u64::from(sample_rate))
            .filter(|ms| *ms > 0)
    });
    let analyzed_duration_ms = if channels == 0 || sample_rate == 0 {
        None
    } else {
        overall_count
            .checked_div(u64::from(channels))
            .and_then(|frames| frames.checked_mul(1_000))
            .and_then(|ms| ms.checked_div(u64::from(sample_rate)))
            .filter(|ms| *ms > 0)
    };

    Ok(AudioLevelSummary {
        channels,
        sample_rate,
        duration_ms: measured_duration_ms,
        analyzed_duration_ms,
        channel_rms_dbfs: channel_sums
            .into_iter()
            .zip(channel_counts)
            .map(|(sum, count)| rms_dbfs(sum, count).map(round_db))
            .collect(),
        overall_rms_dbfs: rms_dbfs(overall_sum, overall_count).map(round_db),
    })
}

fn mixed_audio_quality_from_level(level: &AudioLevelSummary) -> StorageMixedAudioQuality {
    let mic_rms_dbfs = level.channel_rms_dbfs.first().copied().flatten();
    let system_rms_dbfs = level.channel_rms_dbfs.get(1).copied().flatten();
    let mic_minus_system_db = mic_rms_dbfs
        .zip(system_rms_dbfs)
        .map(|(mic, system)| round_db(mic - system));
    let status = if level.channels < 2 {
        "notStereo"
    } else if mic_rms_dbfs.is_none() {
        "micSilent"
    } else if mic_rms_dbfs.is_some_and(|dbfs| dbfs < MIN_MIC_RMS_DBFS) {
        "micTooQuiet"
    } else if system_rms_dbfs.is_some_and(|dbfs| dbfs > AUDIBLE_SYSTEM_RMS_DBFS)
        && mic_minus_system_db.is_some_and(|delta| delta < -MAX_MIC_BELOW_SYSTEM_DB)
    {
        "micTooQuietRelativeToSystem"
    } else {
        "passed"
    };

    StorageMixedAudioQuality {
        status: status.to_string(),
        channels: level.channels,
        sample_rate: level.sample_rate,
        duration_ms: level.duration_ms,
        analyzed_duration_ms: level.analyzed_duration_ms,
        mic_rms_dbfs,
        system_rms_dbfs,
        mic_minus_system_db,
        min_mic_rms_dbfs: MIN_MIC_RMS_DBFS,
        audible_system_rms_dbfs: AUDIBLE_SYSTEM_RMS_DBFS,
        max_mic_below_system_db: MAX_MIC_BELOW_SYSTEM_DB,
    }
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
