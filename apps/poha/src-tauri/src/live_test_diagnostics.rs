use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[path = "live_test_diagnostics_report.rs"]
mod report;
#[path = "live_test_diagnostics_stats.rs"]
mod stats;
#[cfg(test)]
#[path = "live_test_diagnostics_tests.rs"]
mod tests;

pub use report::{LiveTestReport, LiveTestReportInput, build_and_write_report};

pub const REPORT_JSON_FILE: &str = "live-test-report.json";
pub const REPORT_MARKDOWN_FILE: &str = "live-test-report.md";
pub const RUNTIME_NOTE_FILE: &str = "live-test-runtime.json";
pub const DIAGNOSTIC_PHRASE: &str =
    "Poha live test recording microphone and system audio diagnostics.";
pub const GUIDED_MIC_PHRASE: &str = "Poha microphone check";
pub const GUIDED_SYSTEM_PHRASE: &str = "Poha system audio check";
pub const WARMUP_TIMEOUT: Duration = Duration::from_secs(15);
pub const RECORD_AFTER_READY: Duration = Duration::from_secs(8);
pub const GUIDED_MIC_RECORD_SECONDS: Duration = Duration::from_secs(10);
pub const GUIDED_SYSTEM_RECORD_SECONDS: Duration = Duration::from_secs(5);

const MIN_CAPTURE_BYTES: u64 = 4_096;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWarmup {
    pub ready: bool,
    pub waited_ms: u64,
    pub observed_files: Vec<CaptureFileProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureFileProbe {
    pub path: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveTestMode {
    Automatic,
    Guided,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTestRuntimeNote {
    #[serde(default = "default_live_test_mode")]
    pub mode: LiveTestMode,
    pub generated_at: String,
    pub warmup: CaptureWarmup,
    pub diagnostic_phrase: String,
    pub playback_started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_mic_phrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_prompt_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_prompt_ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_prompt_shown: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_prompt_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_playback_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_playback_ended_at: Option<String>,
}

pub async fn wait_for_capture_audio(capture_dir: PathBuf, timeout: Duration) -> CaptureWarmup {
    match tokio::task::spawn_blocking(move || {
        wait_for_capture_audio_blocking(&capture_dir, timeout)
    })
    .await
    {
        Ok(warmup) => warmup,
        Err(error) => CaptureWarmup {
            ready: false,
            waited_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            observed_files: vec![CaptureFileProbe {
                path: format!("capture warmup task failed: {error}"),
                exists: false,
                bytes: None,
            }],
        },
    }
}

fn wait_for_capture_audio_blocking(capture_dir: &Path, timeout: Duration) -> CaptureWarmup {
    let started = Instant::now();
    loop {
        let observed_files = capture_file_probes(capture_dir);
        if observed_files
            .iter()
            .any(|probe| probe.bytes.is_some_and(|bytes| bytes >= MIN_CAPTURE_BYTES))
        {
            return CaptureWarmup {
                ready: true,
                waited_ms: elapsed_ms(started),
                observed_files,
            };
        }
        if started.elapsed() >= timeout {
            return CaptureWarmup {
                ready: false,
                waited_ms: elapsed_ms(started),
                observed_files,
            };
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn play_diagnostic_audio() -> Result<(), String> {
    play_diagnostic_audio_phrase(DIAGNOSTIC_PHRASE)
}

pub fn play_diagnostic_audio_phrase(phrase: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/say")
            .arg(phrase)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed starting diagnostic speech: {error}"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("diagnostic speech playback is only implemented on macOS".to_string())
    }
}

pub fn show_guided_mic_prompt(phrase: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script =
            format!("display notification \"Say: {phrase}\" with title \"Poha Audio Check\"");
        std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed showing guided mic prompt: {error}"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = phrase;
        Err("guided microphone prompt is only implemented on macOS".to_string())
    }
}

pub fn write_runtime_note(output_dir: &Path, note: &LiveTestRuntimeNote) -> Result<(), String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|error| format!("failed creating live test output dir: {error}"))?;
    let path = output_dir.join(RUNTIME_NOTE_FILE);
    let json = serde_json::to_string_pretty(note)
        .map_err(|error| format!("failed serializing live test runtime note: {error}"))?;
    std::fs::write(&path, json)
        .map_err(|error| format!("failed writing {}: {error}", path.display()))
}

pub fn write_permission_failure_report(
    recordings_dir: &Path,
    microphone_permission: bool,
    system_audio_permission: bool,
) -> Result<LiveTestReport, String> {
    let session_id = format!("preflight-{}", uuid::Uuid::new_v4());
    let output_dir = recordings_dir
        .join(".poha")
        .join("live-tests")
        .join(&session_id);
    let capture_dir = output_dir.join("capture");
    std::fs::create_dir_all(&capture_dir)
        .map_err(|error| format!("failed creating live test diagnostics dir: {error}"))?;
    write_runtime_note(
        &output_dir,
        &LiveTestRuntimeNote {
            generated_at: Utc::now().to_rfc3339(),
            warmup: CaptureWarmup {
                ready: false,
                waited_ms: 0,
                observed_files: capture_file_probes(&capture_dir),
            },
            diagnostic_phrase: DIAGNOSTIC_PHRASE.to_string(),
            mode: LiveTestMode::Automatic,
            playback_started: false,
            playback_error: Some("permissions missing before capture started".to_string()),
            user_mic_phrase: None,
            mic_prompt_started_at: None,
            mic_prompt_ended_at: None,
            mic_prompt_shown: None,
            mic_prompt_error: None,
            system_playback_started_at: None,
            system_playback_ended_at: None,
        },
    )?;
    build_and_write_report(LiveTestReportInput {
        session_id,
        output_dir,
        capture_dir,
        manifest_status: "error".to_string(),
        microphone_permission,
        system_audio_permission,
        capture_error: Some("permissions missing before capture started".to_string()),
        transcription_error: None,
    })
}

pub(super) fn read_runtime_note(output_dir: &Path) -> Option<LiveTestRuntimeNote> {
    let path = output_dir.join(RUNTIME_NOTE_FILE);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn capture_file_probes(capture_dir: &Path) -> Vec<CaptureFileProbe> {
    [
        "audio.wav",
        "audio_mic.wav",
        "audio_mic_processed.wav",
        "audio_spk.wav",
    ]
    .iter()
    .map(|name| {
        let path = capture_dir.join(name);
        let bytes = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
        CaptureFileProbe {
            path: path_string(&path),
            exists: bytes.is_some(),
            bytes,
        }
    })
    .collect()
}

fn default_live_test_mode() -> LiveTestMode {
    LiveTestMode::Automatic
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
