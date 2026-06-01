use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::StorageError;

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "caf", "flac", "m4a", "mp3", "mp4", "ogg", "wav",
];
const SOURCE_AUDIO_FILES: &[&str] = &[
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
    "audio.m4a",
    "audio_mic.wav",
    "audio_mic_processed.wav",
    "audio_spk.wav",
];
pub(super) const DELETED_DIR: &str = ".poha/deleted";
pub(super) const TRANSCRIPT_CHUNKS_DIR: &str = "transcript_chunks";
pub(super) const DELETED_RETENTION_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AudioFormatSummary {
    pub(super) extension: String,
    pub(super) bytes: u64,
    pub(super) file_count: u64,
}

#[derive(Debug, Default)]
pub(super) struct ScanSummary {
    pub(super) bytes: u64,
    pub(super) audio_bytes: u64,
    pub(super) audio_file_count: u64,
    pub(super) source_audio_bytes: u64,
    pub(super) source_audio_file_count: u64,
    pub(super) mixed_source_audio_bytes: u64,
    pub(super) mixed_source_audio_file_count: u64,
    pub(super) legacy_wav_source_audio_bytes: u64,
    pub(super) legacy_wav_source_audio_file_count: u64,
    pub(super) stem_wav_bytes: u64,
    pub(super) stem_wav_file_count: u64,
    pub(super) derived_chunk_audio_bytes: u64,
    pub(super) derived_chunk_audio_file_count: u64,
    pub(super) other_audio_bytes: u64,
    pub(super) other_audio_file_count: u64,
    pub(super) audio_by_format: BTreeMap<String, AudioFormatSummary>,
}

pub(super) fn collect_session_dirs(root: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| {
        StorageError::new(
            "recordingsUnavailable",
            format!("failed to read {}: {error}", path_string(root)),
        )
    })? {
        let entry = entry.map_err(|error| {
            StorageError::new(
                "recordingsUnavailable",
                format!("failed to read {} entry: {error}", path_string(root)),
            )
        })?;
        let path = entry.path();
        if path.is_dir() && path.file_name().is_none_or(|name| name != ".poha") {
            dirs.push(path);
        }
    }
    Ok(dirs)
}

pub(super) fn scan_path(path: &Path) -> Result<ScanSummary, StorageError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        StorageError::new(
            "storageScanFailed",
            format!("failed to inspect {}: {error}", path_string(path)),
        )
    })?;
    if metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(scan_file(path, metadata.len()));
    }
    if !metadata.is_dir() {
        return Ok(ScanSummary::default());
    }
    let mut summary = ScanSummary::default();
    for entry in fs::read_dir(path).map_err(|error| {
        StorageError::new(
            "storageScanFailed",
            format!("failed to read {}: {error}", path_string(path)),
        )
    })? {
        let entry = entry.map_err(|error| {
            StorageError::new(
                "storageScanFailed",
                format!("failed to read {} entry: {error}", path_string(path)),
            )
        })?;
        summary.add(scan_path(&entry.path())?);
    }
    Ok(summary)
}

pub(super) fn audio_formats(map: BTreeMap<String, AudioFormatSummary>) -> Vec<AudioFormatSummary> {
    map.into_values().collect()
}

pub(super) fn deleted_session_time(path: &Path) -> Result<DateTime<Utc>, StorageError> {
    if let Some(name) = path.file_name().and_then(|name| name.to_str())
        && name.len() >= 15
        && let Ok(parsed) = NaiveDateTime::parse_from_str(&name[..15], "%Y%m%d-%H%M%S")
    {
        return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
    }
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .map_err(|error| {
            StorageError::new(
                "storageScanFailed",
                format!(
                    "failed reading deleted time for {}: {error}",
                    path_string(path)
                ),
            )
        })
}

pub(super) fn read_json(path: &Path) -> Result<Value, StorageError> {
    let content = fs::read_to_string(path).map_err(|error| {
        StorageError::new(
            "storageScanFailed",
            format!("failed reading {}: {error}", path_string(path)),
        )
    })?;
    serde_json::from_str(&content).map_err(|error| {
        StorageError::new(
            "storageScanFailed",
            format!("failed parsing {}: {error}", path_string(path)),
        )
    })
}

pub(super) fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata_string_exact(metadata, key).or_else(|| {
        metadata_key_alias(key).and_then(|alias| metadata_string_exact(metadata, &alias))
    })
}

pub(super) fn valid_audio_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
        && poha_audio_utils::audio_file_metadata(path).is_ok()
}

pub(super) fn ensure_recordings_dir(recordings_dir: &Path) -> Result<(), StorageError> {
    if recordings_dir.is_dir() {
        Ok(())
    } else {
        Err(StorageError::new(
            "recordingsUnavailable",
            format!("recordings dir not found: {}", path_string(recordings_dir)),
        ))
    }
}

pub(super) fn file_name_string(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_string(path))
}

pub(super) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn scan_file(path: &Path, bytes: u64) -> ScanSummary {
    let mut summary = ScanSummary {
        bytes,
        ..ScanSummary::default()
    };
    if let Some(extension) = audio_extension(path) {
        summary.audio_bytes = bytes;
        summary.audio_file_count = 1;
        if is_transcript_chunk(path) {
            summary.derived_chunk_audio_bytes = bytes;
            summary.derived_chunk_audio_file_count = 1;
        } else if is_source_audio(path) {
            summary.source_audio_bytes = bytes;
            summary.source_audio_file_count = 1;
            if is_stem_wav(path) {
                summary.stem_wav_bytes = bytes;
                summary.stem_wav_file_count = 1;
            } else if file_name_eq(path, "audio.wav") {
                summary.legacy_wav_source_audio_bytes = bytes;
                summary.legacy_wav_source_audio_file_count = 1;
            } else {
                summary.mixed_source_audio_bytes = bytes;
                summary.mixed_source_audio_file_count = 1;
            }
        } else {
            summary.other_audio_bytes = bytes;
            summary.other_audio_file_count = 1;
        }
        summary.audio_by_format.insert(
            extension.to_string(),
            AudioFormatSummary {
                extension: extension.to_string(),
                bytes,
                file_count: 1,
            },
        );
    }
    summary
}

impl ScanSummary {
    fn add(&mut self, other: ScanSummary) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.audio_bytes = self.audio_bytes.saturating_add(other.audio_bytes);
        self.audio_file_count = self.audio_file_count.saturating_add(other.audio_file_count);
        self.source_audio_bytes = self
            .source_audio_bytes
            .saturating_add(other.source_audio_bytes);
        self.source_audio_file_count = self
            .source_audio_file_count
            .saturating_add(other.source_audio_file_count);
        self.mixed_source_audio_bytes = self
            .mixed_source_audio_bytes
            .saturating_add(other.mixed_source_audio_bytes);
        self.mixed_source_audio_file_count = self
            .mixed_source_audio_file_count
            .saturating_add(other.mixed_source_audio_file_count);
        self.legacy_wav_source_audio_bytes = self
            .legacy_wav_source_audio_bytes
            .saturating_add(other.legacy_wav_source_audio_bytes);
        self.legacy_wav_source_audio_file_count = self
            .legacy_wav_source_audio_file_count
            .saturating_add(other.legacy_wav_source_audio_file_count);
        self.stem_wav_bytes = self.stem_wav_bytes.saturating_add(other.stem_wav_bytes);
        self.stem_wav_file_count = self
            .stem_wav_file_count
            .saturating_add(other.stem_wav_file_count);
        self.derived_chunk_audio_bytes = self
            .derived_chunk_audio_bytes
            .saturating_add(other.derived_chunk_audio_bytes);
        self.derived_chunk_audio_file_count = self
            .derived_chunk_audio_file_count
            .saturating_add(other.derived_chunk_audio_file_count);
        self.other_audio_bytes = self
            .other_audio_bytes
            .saturating_add(other.other_audio_bytes);
        self.other_audio_file_count = self
            .other_audio_file_count
            .saturating_add(other.other_audio_file_count);
        for (extension, summary) in other.audio_by_format {
            let entry = self
                .audio_by_format
                .entry(extension.clone())
                .or_insert_with(|| AudioFormatSummary {
                    extension,
                    bytes: 0,
                    file_count: 0,
                });
            entry.bytes = entry.bytes.saturating_add(summary.bytes);
            entry.file_count = entry.file_count.saturating_add(summary.file_count);
        }
    }
}

fn audio_extension(path: &Path) -> Option<&str> {
    let extension = path.extension()?.to_str()?;
    AUDIO_EXTENSIONS
        .iter()
        .copied()
        .find(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn is_source_audio(path: &Path) -> bool {
    SOURCE_AUDIO_FILES
        .iter()
        .any(|candidate| file_name_eq(path, candidate))
}

fn is_stem_wav(path: &Path) -> bool {
    file_name_eq(path, "audio_mic.wav")
        || file_name_eq(path, "audio_mic_processed.wav")
        || file_name_eq(path, "audio_spk.wav")
}

fn is_transcript_chunk(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name == TRANSCRIPT_CHUNKS_DIR)
    })
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn metadata_string_exact(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn metadata_key_alias(key: &str) -> Option<String> {
    if key.contains('_') {
        let mut alias = String::with_capacity(key.len());
        let mut uppercase_next = false;
        for ch in key.chars() {
            if ch == '_' {
                uppercase_next = true;
            } else if uppercase_next {
                alias.push(ch.to_ascii_uppercase());
                uppercase_next = false;
            } else {
                alias.push(ch);
            }
        }
        return (alias != key).then_some(alias);
    }
    None
}
