use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_settings::SettingsPluginExt;

const DEFAULT_MLX_MODEL: &str = "mlx-community/whisper-turbo";
const DEFAULT_MLX_FALLBACK_MODEL: &str = "mlx-community/whisper-turbo";
const LEGACY_PREVIOUS_DEFAULT_MLX_MODEL: &str = "mlx-community/whisper-large-v3-mlx-8bit";
const LEGACY_SLOW_MLX_MODEL: &str = "mlx-community/whisper-large-v3-turbo";
const LEGACY_BROKEN_MLX_MODEL: &str = "mlx-community/whisper-large-v3-turbo-8bit";
const LEGACY_MLX_FALLBACK_MODEL: &str = "mlx-community/whisper-small";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpeakerLabelMode {
    FastMixed,
    MeAndCall,
}

impl Default for SpeakerLabelMode {
    fn default() -> Self {
        Self::MeAndCall
    }
}

impl SpeakerLabelMode {
    pub fn menu_label(self) -> &'static str {
        match self {
            Self::FastMixed => "Fast",
            Self::MeAndCall => "Me + Call",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderSettings {
    pub recordings_dir: String,
    pub mlx_model: String,
    pub mlx_fallback_model: String,
    pub mic_device_id: Option<String>,
    pub preserve_stems: bool,
    #[serde(default)]
    pub speaker_label_mode: SpeakerLabelMode,
    #[serde(default)]
    pub system_audio_authorized_hint: bool,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default = "default_meeting_end_reminders_enabled")]
    pub meeting_end_reminders_enabled: bool,
}

impl RecorderSettings {
    pub fn default_with_recordings_dir(recordings_dir: PathBuf) -> Self {
        Self {
            recordings_dir: recordings_dir.to_string_lossy().into_owned(),
            mlx_model: DEFAULT_MLX_MODEL.to_string(),
            mlx_fallback_model: DEFAULT_MLX_FALLBACK_MODEL.to_string(),
            mic_device_id: None,
            preserve_stems: false,
            speaker_label_mode: SpeakerLabelMode::default(),
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        }
    }

    pub fn recordings_dir_path(&self) -> PathBuf {
        PathBuf::from(&self.recordings_dir)
    }
}

fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .settings()
        .global_base()
        .map_err(|e| format!("settings.global_base failed: {e}"))?;
    Ok(base.join("poha.settings.json").into_std_path_buf())
}

fn default_recordings_dir() -> PathBuf {
    let Some(home) = dirs::home_dir() else {
        return PathBuf::from("./recordings");
    };
    home.join("Library")
        .join("Application Support")
        .join("Poha")
        .join("recordings")
}

pub fn load(app: &AppHandle) -> Result<RecorderSettings, String> {
    let path = settings_file(app)?;

    if !path.exists() {
        let default = RecorderSettings::default_with_recordings_dir(default_recordings_dir());
        save(app, &default)?;
        return Ok(default);
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed reading settings file {}: {e}", path.display()))?;
    let raw_settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("invalid settings json {}: {e}", path.display()))?;
    let missing_defaults = missing_persisted_defaults(&raw_settings);
    let settings: RecorderSettings = serde_json::from_value(raw_settings)
        .map_err(|e| format!("invalid settings json {}: {e}", path.display()))?;
    let (settings, changed) = migrate_settings(settings);
    if changed || missing_defaults {
        save(app, &settings)?;
    }
    Ok(settings)
}

pub fn default_settings_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("Application Support")
        .join("Poha")
        .join("poha.settings.json")
}

pub fn load_from_file(path: &Path, recordings_dir: PathBuf) -> Result<RecorderSettings, String> {
    if !path.exists() {
        return Ok(RecorderSettings::default_with_recordings_dir(
            recordings_dir,
        ));
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed reading settings file {}: {e}", path.display()))?;
    let raw_settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("invalid settings json {}: {e}", path.display()))?;
    let settings: RecorderSettings = serde_json::from_value(raw_settings)
        .map_err(|e| format!("invalid settings json {}: {e}", path.display()))?;
    let (mut settings, _) = migrate_settings(settings);
    settings.recordings_dir = recordings_dir.to_string_lossy().into_owned();
    Ok(settings)
}

pub fn save(app: &AppHandle, settings: &RecorderSettings) -> Result<(), String> {
    let path = settings_file(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed creating settings dir {}: {e}", parent.display()))?;
    }

    let data = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("failed serializing settings: {e}"))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("failed writing settings file {}: {e}", path.display()))?;
    Ok(())
}

pub fn latest_transcript_path(recordings_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(recordings_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let transcript = session_dir.join("transcript.md");
        if !transcript.exists() {
            continue;
        }
        let modified = transcript.metadata().ok()?.modified().ok()?;
        match &best {
            Some((current_modified, _)) if modified <= *current_modified => {}
            _ => best = Some((modified, transcript)),
        }
    }

    best.map(|(_, path)| path)
}

fn migrate_settings(mut settings: RecorderSettings) -> (RecorderSettings, bool) {
    let mut changed = false;
    let model = settings.mlx_model.trim().to_string();

    let using_legacy_slow_model = model.is_empty()
        || model == LEGACY_PREVIOUS_DEFAULT_MLX_MODEL
        || model == LEGACY_SLOW_MLX_MODEL
        || model == LEGACY_BROKEN_MLX_MODEL;
    if using_legacy_slow_model {
        settings.mlx_model = DEFAULT_MLX_MODEL.to_string();
        changed = true;
    }

    if settings.mlx_fallback_model.trim().is_empty()
        || settings.mlx_fallback_model == LEGACY_MLX_FALLBACK_MODEL
    {
        settings.mlx_fallback_model = DEFAULT_MLX_FALLBACK_MODEL.to_string();
        changed = true;
    }

    if using_legacy_slow_model && settings.preserve_stems {
        settings.preserve_stems = false;
        changed = true;
    }

    (settings, changed)
}

fn missing_persisted_defaults(settings: &serde_json::Value) -> bool {
    settings.get("speakerLabelMode").is_none()
        || settings.get("systemAudioAuthorizedHint").is_none()
        || settings.get("onboardingCompleted").is_none()
        || settings.get("meetingEndRemindersEnabled").is_none()
}

fn default_meeting_end_reminders_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_previous_default_model_to_whisper_turbo() {
        let legacy = RecorderSettings {
            recordings_dir: "/tmp/recordings".to_string(),
            mlx_model: LEGACY_PREVIOUS_DEFAULT_MLX_MODEL.to_string(),
            mlx_fallback_model: DEFAULT_MLX_FALLBACK_MODEL.to_string(),
            mic_device_id: None,
            preserve_stems: true,
            speaker_label_mode: SpeakerLabelMode::MeAndCall,
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        };
        let (migrated, changed) = migrate_settings(legacy);
        assert!(changed);
        assert_eq!(migrated.mlx_model, DEFAULT_MLX_MODEL);
        assert!(!migrated.preserve_stems);
    }

    #[test]
    fn migrates_legacy_slow_model_to_whisper_turbo() {
        let previous = RecorderSettings {
            recordings_dir: "/tmp/recordings".to_string(),
            mlx_model: LEGACY_SLOW_MLX_MODEL.to_string(),
            mlx_fallback_model: DEFAULT_MLX_FALLBACK_MODEL.to_string(),
            mic_device_id: None,
            preserve_stems: true,
            speaker_label_mode: SpeakerLabelMode::MeAndCall,
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        };
        let (migrated, changed) = migrate_settings(previous);
        assert!(changed);
        assert_eq!(migrated.mlx_model, DEFAULT_MLX_MODEL);
        assert!(!migrated.preserve_stems);
    }

    #[test]
    fn migrates_broken_8bit_model_to_whisper_turbo() {
        let broken = RecorderSettings {
            recordings_dir: "/tmp/recordings".to_string(),
            mlx_model: LEGACY_BROKEN_MLX_MODEL.to_string(),
            mlx_fallback_model: DEFAULT_MLX_FALLBACK_MODEL.to_string(),
            mic_device_id: None,
            preserve_stems: true,
            speaker_label_mode: SpeakerLabelMode::MeAndCall,
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        };
        let (migrated, changed) = migrate_settings(broken);
        assert!(changed);
        assert_eq!(migrated.mlx_model, DEFAULT_MLX_MODEL);
        assert!(!migrated.preserve_stems);
    }

    #[test]
    fn preserves_explicit_custom_model() {
        let custom = RecorderSettings {
            recordings_dir: "/tmp/recordings".to_string(),
            mlx_model: "knownsense/whisper-hindi-apex-mlx".to_string(),
            mlx_fallback_model: "mlx-community/whisper-turbo".to_string(),
            mic_device_id: None,
            preserve_stems: true,
            speaker_label_mode: SpeakerLabelMode::MeAndCall,
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        };
        let (migrated, changed) = migrate_settings(custom.clone());
        assert!(!changed);
        assert_eq!(migrated.mlx_model, custom.mlx_model);
    }

    #[test]
    fn migrates_legacy_fallback_model() {
        let legacy = RecorderSettings {
            recordings_dir: "/tmp/recordings".to_string(),
            mlx_model: DEFAULT_MLX_MODEL.to_string(),
            mlx_fallback_model: LEGACY_MLX_FALLBACK_MODEL.to_string(),
            mic_device_id: None,
            preserve_stems: true,
            speaker_label_mode: SpeakerLabelMode::MeAndCall,
            system_audio_authorized_hint: false,
            onboarding_completed: false,
            meeting_end_reminders_enabled: true,
        };
        let (migrated, changed) = migrate_settings(legacy);
        assert!(changed);
        assert_eq!(migrated.mlx_fallback_model, DEFAULT_MLX_FALLBACK_MODEL);
    }

    #[test]
    fn missing_system_audio_hint_defaults_false() {
        let json = r#"{
            "recordingsDir":"/tmp/recordings",
            "mlxModel":"mlx-community/whisper-turbo",
            "mlxFallbackModel":"mlx-community/whisper-turbo",
            "micDeviceId":null,
            "preserveStems":false
        }"#;

        let parsed: RecorderSettings = serde_json::from_str(json).expect("parse settings");
        assert!(!parsed.system_audio_authorized_hint);
    }

    #[test]
    fn missing_onboarding_completed_defaults_false() {
        let json = r#"{
            "recordingsDir":"/tmp/recordings",
            "mlxModel":"mlx-community/whisper-turbo",
            "mlxFallbackModel":"mlx-community/whisper-turbo",
            "micDeviceId":null,
            "preserveStems":false
        }"#;

        let parsed: RecorderSettings = serde_json::from_str(json).expect("parse settings");
        assert!(!parsed.onboarding_completed);
    }

    #[test]
    fn missing_speaker_label_mode_defaults_to_me_and_call() {
        let json = r#"{
            "recordingsDir":"/tmp/recordings",
            "mlxModel":"mlx-community/whisper-turbo",
            "mlxFallbackModel":"mlx-community/whisper-turbo",
            "micDeviceId":null,
            "preserveStems":false
        }"#;

        let parsed: RecorderSettings = serde_json::from_str(json).expect("parse settings");
        assert_eq!(parsed.speaker_label_mode, SpeakerLabelMode::MeAndCall);
    }

    #[test]
    fn missing_meeting_end_reminders_defaults_enabled() {
        let json = r#"{
            "recordingsDir":"/tmp/recordings",
            "mlxModel":"mlx-community/whisper-turbo",
            "mlxFallbackModel":"mlx-community/whisper-turbo",
            "micDeviceId":null,
            "preserveStems":false
        }"#;

        let parsed: RecorderSettings = serde_json::from_str(json).expect("parse settings");
        assert!(parsed.meeting_end_reminders_enabled);
    }

    #[test]
    fn missing_persisted_defaults_are_detected() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "recordingsDir":"/tmp/recordings",
                "mlxModel":"mlx-community/whisper-turbo",
                "mlxFallbackModel":"mlx-community/whisper-turbo",
                "micDeviceId":null,
                "preserveStems":false
            }"#,
        )
        .expect("parse settings");

        assert!(missing_persisted_defaults(&json));
    }
}
