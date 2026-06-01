use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

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
    pub audio_path: Option<String>,
    pub transcript_path: Option<String>,
    pub transcription: Option<TranscriptionMetadata>,
    pub error: Option<String>,
}

impl SessionManifest {
    pub fn recording(id: String, capture_dir: String) -> Self {
        Self {
            id,
            status: "recording".to_string(),
            started_at: Utc::now().to_rfc3339(),
            ended_at: None,
            capture_dir,
            audio_path: None,
            transcript_path: None,
            transcription: None,
            error: None,
        }
    }

    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
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
        self.audio_path = Some(audio_path);
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

pub fn manifest_path(session_output_dir: &Path) -> std::path::PathBuf {
    session_output_dir.join("session.json")
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
    let data = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("failed serializing manifest: {e}"))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("failed writing manifest {}: {e}", path.display()))
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
    }
}
