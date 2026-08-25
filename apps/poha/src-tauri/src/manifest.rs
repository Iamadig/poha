use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::recorder_settings::RecordingMode;

const MANIFEST_FILE_NAME: &str = "session.json";
const MANIFEST_TEMP_PREFIX: &str = ".session.json.";
const MANIFEST_TEMP_SUFFIX: &str = ".tmp";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioArtifactPaths {
    pub mixed_audio_path: Option<String>,
    pub microphone_audio_path: Option<String>,
    pub processed_microphone_audio_path: Option<String>,
    pub system_audio_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionMetadata {
    pub engine: String,
    pub status: String,
    pub model: String,
    pub fallback_model: String,
    pub transcript_json_path: String,
    pub transcript_markdown_path: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionManifest {
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub capture_dir: String,
    #[serde(default = "legacy_recording_mode")]
    pub recording_mode: RecordingMode,
    #[serde(default)]
    pub preserve_stems: bool,
    #[serde(default)]
    pub audio_artifacts: AudioArtifactPaths,
    pub audio_path: Option<String>,
    pub transcript_path: Option<String>,
    pub transcription: Option<TranscriptionMetadata>,
    pub error: Option<String>,
}

impl SessionManifest {
    #[cfg(test)]
    pub fn recording(id: String, capture_dir: String) -> Self {
        Self::recording_with_policy(id, capture_dir, RecordingMode::default(), true)
    }

    pub fn recording_with_policy(
        id: String,
        capture_dir: String,
        recording_mode: RecordingMode,
        preserve_stems: bool,
    ) -> Self {
        Self {
            id,
            status: "recording".to_string(),
            started_at: Utc::now().to_rfc3339(),
            ended_at: None,
            capture_dir,
            recording_mode,
            preserve_stems,
            audio_artifacts: AudioArtifactPaths::default(),
            audio_path: None,
            transcript_path: None,
            transcription: None,
            error: None,
        }
    }

    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
    }

    pub fn set_audio_artifacts(&mut self, audio_artifacts: AudioArtifactPaths) {
        if let Some(mixed_audio_path) = audio_artifacts.mixed_audio_path.clone() {
            self.audio_path = Some(mixed_audio_path);
        }
        self.audio_artifacts = audio_artifacts;
    }

    pub fn mark_recorded(&mut self, audio_artifacts: AudioArtifactPaths) {
        self.status = "recorded".to_string();
        self.ended_at = Some(Utc::now().to_rfc3339());
        self.set_audio_artifacts(audio_artifacts);
        self.transcript_path = None;
        self.transcription = None;
        self.error = None;
    }

    pub fn mark_done(
        &mut self,
        audio_path: String,
        transcript_markdown_path: String,
        transcript_json_path: String,
        model: String,
        fallback_model: String,
    ) {
        let updated_at = Utc::now().to_rfc3339();
        self.status = "done".to_string();
        self.ended_at = Some(updated_at.clone());
        self.audio_path = Some(audio_path.clone());
        self.audio_artifacts.mixed_audio_path = Some(audio_path);
        self.transcript_path = Some(transcript_markdown_path.clone());
        self.error = None;
        self.transcription = Some(TranscriptionMetadata {
            engine: "mlx-whisper".to_string(),
            status: "done".to_string(),
            model,
            fallback_model,
            transcript_json_path,
            transcript_markdown_path,
            updated_at,
        });
    }

    pub fn mark_error(&mut self, error: String) {
        self.status = "error".to_string();
        self.ended_at = Some(Utc::now().to_rfc3339());
        self.error = Some(error);
        if let Some(transcription) = self.transcription.as_mut() {
            transcription.status = "error".to_string();
            transcription.updated_at = Utc::now().to_rfc3339();
        }
    }
}

fn legacy_recording_mode() -> RecordingMode {
    RecordingMode::RecordAndTranscribe
}

pub fn manifest_path(session_output_dir: &Path) -> std::path::PathBuf {
    session_output_dir.join(MANIFEST_FILE_NAME)
}

pub fn read_manifest(session_output_dir: &Path) -> Result<SessionManifest, String> {
    let path = manifest_path(session_output_dir);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed reading manifest {}: {e}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("failed parsing manifest {}: {e}", path.display()))
}

pub fn write_manifest(session_output_dir: &Path, manifest: &SessionManifest) -> Result<(), String> {
    std::fs::create_dir_all(session_output_dir).map_err(|e| {
        format!(
            "failed creating session output dir {}: {e}",
            session_output_dir.display()
        )
    })?;
    let path = manifest_path(session_output_dir);
    let temporary_path = temporary_manifest_path(session_output_dir);
    let data = serde_json::to_vec_pretty(manifest)
        .map_err(|e| format!("failed serializing manifest: {e}"))?;

    let mut temporary = open_temporary_manifest(&temporary_path).map_err(|e| {
        format!(
            "failed creating temporary manifest {}: {e}",
            temporary_path.display()
        )
    })?;
    let write_result = (|| {
        temporary.write_all(&data).map_err(|e| {
            format!(
                "failed writing temporary manifest {}: {e}",
                temporary_path.display()
            )
        })?;
        temporary.sync_all().map_err(|e| {
            format!(
                "failed syncing temporary manifest {}: {e}",
                temporary_path.display()
            )
        })
    })();
    drop(temporary);

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }

    if let Err(error) = std::fs::rename(&temporary_path, &path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(format!(
            "failed replacing manifest {} with {}: {error}",
            path.display(),
            temporary_path.display()
        ));
    }

    sync_directory(session_output_dir).map_err(|e| {
        format!(
            "manifest {} was replaced but the session directory could not be synced: {e}",
            path.display()
        )
    })
}

fn temporary_manifest_path(session_output_dir: &Path) -> PathBuf {
    session_output_dir.join(format!(
        "{MANIFEST_TEMP_PREFIX}{}{MANIFEST_TEMP_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ))
}

fn open_temporary_manifest(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn manifest_path_is_session_json() {
        let dir = Path::new("/tmp/example-session");
        assert_eq!(manifest_path(dir), dir.join("session.json"));
    }

    #[test]
    fn write_and_read_manifest_roundtrip() {
        let output = tempdir().expect("temp output");
        let mut manifest = SessionManifest::recording(
            "session-1".to_string(),
            "/tmp/capture/session-1".to_string(),
        );
        manifest.set_status("transcribing");

        write_manifest(output.path(), &manifest).expect("write manifest");
        let written_path = manifest_path(output.path());
        assert!(written_path.exists());

        let loaded = read_manifest(output.path()).expect("read manifest");
        assert_eq!(loaded.id, "session-1");
        assert_eq!(loaded.status, "transcribing");
        assert_eq!(loaded.capture_dir, "/tmp/capture/session-1");
        assert_eq!(loaded.recording_mode, RecordingMode::RecordOnly);
        assert!(loaded.preserve_stems);
        assert!(temporary_manifest_paths(output.path()).is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&written_path)
                .expect("manifest metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn replacing_manifest_is_atomic_and_leaves_valid_json() {
        let output = tempdir().expect("temp output");
        let mut manifest = SessionManifest::recording(
            "session-1".to_string(),
            "/tmp/capture/session-1".to_string(),
        );
        write_manifest(output.path(), &manifest).expect("write initial manifest");

        manifest.set_status("recorded");
        write_manifest(output.path(), &manifest).expect("replace manifest");

        let raw = std::fs::read_to_string(manifest_path(output.path())).expect("manifest JSON");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(value["status"], "recorded");
        assert!(temporary_manifest_paths(output.path()).is_empty());
    }

    #[test]
    fn failed_replace_cleans_up_temporary_manifest() {
        let output = tempdir().expect("temp output");
        std::fs::create_dir(manifest_path(output.path())).expect("blocking manifest directory");
        let manifest = SessionManifest::recording(
            "session-1".to_string(),
            "/tmp/capture/session-1".to_string(),
        );

        let error = write_manifest(output.path(), &manifest).expect_err("replace must fail");

        assert!(error.contains("failed replacing manifest"));
        assert!(manifest_path(output.path()).is_dir());
        assert!(temporary_manifest_paths(output.path()).is_empty());
    }

    fn temporary_manifest_paths(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read session directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(MANIFEST_TEMP_PREFIX)
                            && name.ends_with(MANIFEST_TEMP_SUFFIX)
                    })
            })
            .collect()
    }

    #[test]
    fn legacy_manifest_defaults_to_transcription_and_non_preserving_policy() {
        let legacy = r#"{
            "id":"session-1",
            "status":"done",
            "startedAt":"2026-06-01T10:00:00Z",
            "endedAt":"2026-06-01T11:00:00Z",
            "captureDir":"/tmp/capture/session-1",
            "audioPath":"/tmp/recordings/session-1/audio.mp3",
            "transcriptPath":"/tmp/recordings/session-1/transcript.md",
            "transcription":null,
            "error":null
        }"#;

        let manifest: SessionManifest = serde_json::from_str(legacy).expect("legacy manifest");

        assert_eq!(manifest.recording_mode, RecordingMode::RecordAndTranscribe);
        assert!(!manifest.preserve_stems);
        assert_eq!(manifest.audio_artifacts, AudioArtifactPaths::default());
    }

    #[test]
    fn mark_recorded_persists_all_durable_audio_paths() {
        let mut manifest = SessionManifest::recording(
            "session-1".to_string(),
            "/tmp/capture/session-1".to_string(),
        );
        let artifacts = AudioArtifactPaths {
            mixed_audio_path: Some("/recordings/session-1/audio.mp3".to_string()),
            microphone_audio_path: Some("/recordings/session-1/audio_mic.wav".to_string()),
            processed_microphone_audio_path: Some(
                "/recordings/session-1/audio_mic_processed.wav".to_string(),
            ),
            system_audio_path: Some("/recordings/session-1/audio_spk.wav".to_string()),
        };

        manifest.mark_recorded(artifacts.clone());

        assert_eq!(manifest.status, "recorded");
        assert!(manifest.ended_at.is_some());
        assert_eq!(manifest.audio_artifacts, artifacts);
        assert_eq!(
            manifest.audio_path.as_deref(),
            Some("/recordings/session-1/audio.mp3")
        );
        assert!(manifest.transcription.is_none());
        assert!(manifest.transcript_path.is_none());
    }

    #[test]
    fn mark_done_preserves_stem_paths_and_sets_legacy_mixed_path() {
        let mut manifest = SessionManifest::recording(
            "session-1".to_string(),
            "/tmp/capture/session-1".to_string(),
        );
        manifest.audio_artifacts.microphone_audio_path =
            Some("/recordings/session-1/audio_mic.wav".to_string());

        manifest.mark_done(
            "/recordings/session-1/audio.mp3".to_string(),
            "/recordings/session-1/transcript.md".to_string(),
            "/recordings/session-1/transcript.json".to_string(),
            "model".to_string(),
            "fallback".to_string(),
        );

        assert_eq!(
            manifest.audio_artifacts.mixed_audio_path.as_deref(),
            Some("/recordings/session-1/audio.mp3")
        );
        assert_eq!(
            manifest.audio_artifacts.microphone_audio_path.as_deref(),
            Some("/recordings/session-1/audio_mic.wav")
        );
    }
}
