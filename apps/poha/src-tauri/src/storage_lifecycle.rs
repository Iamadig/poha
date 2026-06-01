use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "storage_lifecycle_audio_quality.rs"]
mod audio_quality;
#[path = "storage_lifecycle_audio_quality_report.rs"]
mod audio_quality_report;
#[path = "storage_lifecycle_ledger.rs"]
mod ledger;
#[path = "storage_lifecycle_plan.rs"]
mod plan;
#[path = "storage_lifecycle_report.rs"]
mod report;
#[path = "storage_lifecycle_scan.rs"]
mod scan;
#[path = "storage_lifecycle_validation.rs"]
mod validation;

pub use audio_quality_report::{
    DEFAULT_AUDIO_QUALITY_ANALYSIS_WINDOW_SECONDS, StorageAudioQualityReport, audio_quality_report,
    audio_quality_report_with_options,
};
pub use report::{StorageReport, report};

use scan::{ensure_recordings_dir, metadata_string, path_string, read_json, valid_audio_file};
use validation::{
    StorageCandidateKind, StorageValidationFacts, chunk_manifest_validation, transcript_validation,
    validation_facts,
};

#[derive(Debug, Clone)]
pub struct StorageError {
    code: &'static str,
    message: String,
}

impl StorageError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMaintenancePlan {
    recordings_dir: String,
    generated_at: String,
    dry_run: bool,
    totals: StorageMaintenanceTotals,
    actions: Vec<StorageMaintenanceAction>,
    skipped: Vec<StorageMaintenanceSkip>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageMaintenanceTotals {
    reclaimable_bytes: u64,
    candidate_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageMaintenanceAction {
    kind: String,
    session_id: Option<String>,
    path: String,
    bytes: u64,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    validation_facts: Option<StorageValidationFacts>,
    #[serde(skip)]
    validation_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageMaintenanceSkip {
    session_id: Option<String>,
    path: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMaintenanceResult {
    recordings_dir: String,
    generated_at: String,
    totals: StorageMaintenanceTotals,
    moved_to_trash: Vec<StorageMaintenanceAction>,
    skipped: Vec<StorageMaintenanceSkip>,
    errors: Vec<StorageMaintenanceSkip>,
}

impl StorageMaintenanceResult {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.totals.reclaimable_bytes
    }

    pub fn moved_to_trash_count(&self) -> usize {
        self.moved_to_trash.len()
    }

    pub fn error_count(&self) -> usize {
        self.errors.len()
    }
}

pub trait TrashSink {
    fn move_to_trash(&self, path: &Path) -> Result<(), StorageError>;
}

pub struct SystemTrash;

impl TrashSink for SystemTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), StorageError> {
        system_move_to_trash(path)
    }
}

pub fn maintenance_plan(
    recordings_dir: &Path,
    now: DateTime<Utc>,
) -> Result<StorageMaintenancePlan, StorageError> {
    ensure_recordings_dir(recordings_dir)?;
    let (actions, skipped) = plan::collect_actions(recordings_dir, now)?;

    let totals = StorageMaintenanceTotals {
        reclaimable_bytes: actions.iter().map(|action| action.bytes).sum(),
        candidate_count: actions.len(),
    };

    Ok(StorageMaintenancePlan {
        recordings_dir: path_string(recordings_dir),
        generated_at: now.to_rfc3339(),
        dry_run: true,
        totals,
        actions,
        skipped,
    })
}

pub fn maintain(recordings_dir: &Path) -> Result<StorageMaintenanceResult, StorageError> {
    maintain_with_trash(recordings_dir, Utc::now(), &SystemTrash)
}

pub fn maintain_session(
    recordings_dir: &Path,
    session_dir: &Path,
) -> Result<StorageMaintenanceResult, StorageError> {
    maintain_session_with_trash(recordings_dir, session_dir, Utc::now(), &SystemTrash)
}

pub fn maintain_with_trash(
    recordings_dir: &Path,
    now: DateTime<Utc>,
    trash: &dyn TrashSink,
) -> Result<StorageMaintenanceResult, StorageError> {
    let plan = maintenance_plan(recordings_dir, now)?;
    execute_actions(recordings_dir, now, plan.actions, plan.skipped, trash)
}

pub fn maintain_session_with_trash(
    recordings_dir: &Path,
    session_dir: &Path,
    now: DateTime<Utc>,
    trash: &dyn TrashSink,
) -> Result<StorageMaintenanceResult, StorageError> {
    ensure_recordings_dir(recordings_dir)?;
    ensure_session_dir_in_recordings(recordings_dir, session_dir)?;
    let (actions, skipped) = plan::collect_actions_for_session(recordings_dir, session_dir)?;
    execute_actions(recordings_dir, now, actions, skipped, trash)
}

fn execute_actions(
    recordings_dir: &Path,
    now: DateTime<Utc>,
    actions: Vec<StorageMaintenanceAction>,
    skipped: Vec<StorageMaintenanceSkip>,
    trash: &dyn TrashSink,
) -> Result<StorageMaintenanceResult, StorageError> {
    let mut moved_to_trash = Vec::new();
    let mut skipped = skipped;
    let mut errors = Vec::new();
    let mut blocked_sessions = BTreeSet::new();

    for action in actions {
        if action
            .session_id
            .as_ref()
            .is_some_and(|session_id| blocked_sessions.contains(session_id))
        {
            skipped.push(StorageMaintenanceSkip {
                session_id: action.session_id.clone(),
                path: action.path.clone(),
                reason: "blocked by earlier maintenance failure".to_string(),
            });
            continue;
        }
        let skipped_action = action.clone();
        match execute_action(action, trash) {
            Ok(Some(moved)) => {
                if let Err(error) = append_ledger(recordings_dir, now, &moved) {
                    errors.push(StorageMaintenanceSkip {
                        session_id: moved.session_id.clone(),
                        path: moved.path.clone(),
                        reason: error.message().to_string(),
                    });
                }
                moved_to_trash.push(moved);
            }
            Ok(None) => skipped.push(StorageMaintenanceSkip {
                session_id: skipped_action.session_id.clone(),
                path: skipped_action.path.clone(),
                reason: "candidate no longer exists".to_string(),
            }),
            Err(error) => {
                if skipped_action.kind == "legacyWavTranscode"
                    && let Some(session_id) = skipped_action.session_id.clone()
                {
                    blocked_sessions.insert(session_id);
                }
                errors.push(StorageMaintenanceSkip {
                    session_id: skipped_action.session_id.clone(),
                    path: skipped_action.path.clone(),
                    reason: error.message().to_string(),
                });
            }
        }
    }

    let totals = StorageMaintenanceTotals {
        reclaimable_bytes: moved_to_trash.iter().map(|action| action.bytes).sum(),
        candidate_count: moved_to_trash.len(),
    };

    Ok(StorageMaintenanceResult {
        recordings_dir: path_string(recordings_dir),
        generated_at: now.to_rfc3339(),
        totals,
        moved_to_trash,
        skipped,
        errors,
    })
}

fn ensure_session_dir_in_recordings(
    recordings_dir: &Path,
    session_dir: &Path,
) -> Result<(), StorageError> {
    if !session_dir.is_dir() {
        return Err(StorageError::new(
            "storageSessionUnavailable",
            format!("session dir not found: {}", path_string(session_dir)),
        ));
    }
    let recordings_dir = fs::canonicalize(recordings_dir).map_err(|error| {
        StorageError::new(
            "recordingsUnavailable",
            format!(
                "failed resolving recordings dir {}: {error}",
                path_string(recordings_dir)
            ),
        )
    })?;
    let session_dir = fs::canonicalize(session_dir).map_err(|error| {
        StorageError::new(
            "storageSessionUnavailable",
            format!(
                "failed resolving session dir {}: {error}",
                path_string(session_dir)
            ),
        )
    })?;
    if session_dir == recordings_dir || !session_dir.starts_with(&recordings_dir) {
        return Err(StorageError::new(
            "storageSessionOutsideRecordings",
            format!(
                "session dir is outside recordings dir: {}",
                path_string(&session_dir)
            ),
        ));
    }
    Ok(())
}

fn execute_action(
    mut action: StorageMaintenanceAction,
    trash: &dyn TrashSink,
) -> Result<Option<StorageMaintenanceAction>, StorageError> {
    let path = PathBuf::from(&action.path);
    if !path.exists() {
        return Ok(None);
    }
    if action.kind == "legacyWavTranscode" {
        ensure_mp3_for_legacy_wav(&path)?;
    }
    if let Some(facts) = refreshed_validation_facts(&action)? {
        action.validation_facts = Some(facts);
    }
    trash.move_to_trash(&path)?;
    Ok(Some(action))
}

fn ensure_mp3_for_legacy_wav(wav_path: &Path) -> Result<(), StorageError> {
    let mp3_path = wav_path.with_file_name("audio.mp3");
    if valid_audio_file(&mp3_path) {
        return Ok(());
    }
    let tmp_path = wav_path.with_file_name(format!("audio.{}.tmp.mp3", std::process::id()));
    if tmp_path.exists() {
        return Err(StorageError::new(
            "storageOptimizeFailed",
            format!("temporary mp3 already exists: {}", path_string(&tmp_path)),
        ));
    }
    poha_mp3::encode_wav_mastered(wav_path, &tmp_path).map_err(|error| {
        StorageError::new(
            "storageOptimizeFailed",
            format!("failed encoding {}: {error}", path_string(wav_path)),
        )
    })?;
    if !valid_audio_file(&tmp_path) {
        return Err(StorageError::new(
            "storageOptimizeFailed",
            format!("encoded mp3 failed validation: {}", path_string(&tmp_path)),
        ));
    }
    fs::rename(&tmp_path, &mp3_path).map_err(|error| {
        StorageError::new(
            "storageOptimizeFailed",
            format!(
                "failed moving {} to {}: {error}",
                path_string(&tmp_path),
                path_string(&mp3_path)
            ),
        )
    })
}

fn refreshed_validation_facts(
    action: &StorageMaintenanceAction,
) -> Result<Option<StorageValidationFacts>, StorageError> {
    let kind = match action.kind.as_str() {
        "legacyWav" => StorageCandidateKind::LegacyWav,
        "legacyWavTranscode" => StorageCandidateKind::LegacyWavTranscode,
        "stemWav" => StorageCandidateKind::StemWav,
        "captureScratchAudio" => StorageCandidateKind::CaptureScratchAudio,
        "transcriptChunks" => StorageCandidateKind::TranscriptChunks,
        _ => return Ok(None),
    };
    let path = PathBuf::from(&action.path);
    let validation_dir = action
        .validation_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| path.parent().map(PathBuf::from))
        .ok_or_else(|| {
            StorageError::new(
                "storageMaintenanceInvalidPath",
                "missing validation session dir",
            )
        })?;
    let session_dir = validation_dir.as_path();
    let metadata = read_json(&session_dir.join("session.json")).ok();
    if metadata
        .as_ref()
        .and_then(|value| metadata_string(value, "status"))
        .as_deref()
        != Some("done")
    {
        return Err(StorageError::new(
            "storageMaintenanceUnsafe",
            "session is no longer finalized",
        ));
    }
    let transcript = transcript_validation(session_dir).ok_or_else(|| {
        StorageError::new(
            "storageMaintenanceUnsafe",
            "final transcript is missing or invalid",
        )
    })?;
    let chunk_state = if kind == StorageCandidateKind::TranscriptChunks {
        let state = chunk_manifest_validation(session_dir);
        if !state.is_complete {
            return Err(StorageError::new("storageMaintenanceUnsafe", state.state));
        }
        Some(state.state)
    } else {
        None
    };
    validation_facts(
        session_dir,
        metadata.as_ref(),
        &transcript,
        kind.is_source_audio().then_some(path.as_path()),
        kind,
        chunk_state,
        false,
    )
    .map(Some)
    .map_err(|error| StorageError::new("storageMaintenanceUnsafe", error))
}

fn append_ledger(
    recordings_dir: &Path,
    generated_at: DateTime<Utc>,
    action: &StorageMaintenanceAction,
) -> Result<(), StorageError> {
    let ledger_path = recordings_dir
        .join(".poha")
        .join("storage-maintenance.jsonl");
    if let Some(parent) = ledger_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            StorageError::new(
                "storageLedgerFailed",
                format!("failed creating {}: {error}", path_string(parent)),
            )
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger_path)
        .map_err(|error| {
            StorageError::new(
                "storageLedgerFailed",
                format!("failed opening {}: {error}", path_string(&ledger_path)),
            )
        })?;
    ledger::write_entry(&mut file, generated_at, action)
}

#[cfg(target_os = "macos")]
fn system_move_to_trash(path: &Path) -> Result<(), StorageError> {
    let script = format!(
        "tell application \"Finder\" to delete POSIX file \"{}\"",
        applescript_escape(&path_string(path))
    );
    let status = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|error| {
            StorageError::new(
                "storageTrashFailed",
                format!(
                    "failed launching osascript for {}: {error}",
                    path_string(path)
                ),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(StorageError::new(
            "storageTrashFailed",
            format!("failed moving {} to Trash: {status}", path_string(path)),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn system_move_to_trash(path: &Path) -> Result<(), StorageError> {
    Err(StorageError::new(
        "storageTrashUnavailable",
        format!(
            "system Trash is only implemented on macOS: {}",
            path_string(path)
        ),
    ))
}

#[cfg(target_os = "macos")]
fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
#[path = "storage_lifecycle_tests.rs"]
mod tests;
