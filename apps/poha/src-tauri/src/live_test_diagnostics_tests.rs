use std::path::Path;

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tempfile::tempdir;

use super::*;

#[test]
fn report_passes_for_valid_live_test_artifacts() {
    let dir = tempdir().unwrap();
    let output_dir = dir.path().join("recording");
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&capture_dir).unwrap();
    write_stereo_wav(&output_dir.join("source.wav"), 6, 0.25, 0.35);
    poha_mp3::encode_wav(
        &output_dir.join("source.wav"),
        &output_dir.join("audio.mp3"),
    )
    .unwrap();
    write_mono_wav(&output_dir.join("audio_mic.wav"), 6, 0.25);
    write_mono_wav(&output_dir.join("audio_mic_processed.wav"), 6, 0.22);
    write_mono_wav(&output_dir.join("audio_spk.wav"), 6, 0.35);
    write_transcript(
        &output_dir,
        "Poha live test recording microphone and system audio.",
    );
    let stopped_at = Utc::now();
    write_session_json(&output_dir, stopped_at - Duration::seconds(7));
    write_runtime_note(
        &output_dir,
        &LiveTestRuntimeNote {
            mode: LiveTestMode::Automatic,
            generated_at: stopped_at.to_rfc3339(),
            warmup: CaptureWarmup {
                ready: true,
                waited_ms: 500,
                observed_files: vec![CaptureFileProbe {
                    path: path_string(&capture_dir.join("audio.wav")),
                    exists: true,
                    bytes: Some(MIN_CAPTURE_BYTES),
                }],
            },
            diagnostic_phrase: DIAGNOSTIC_PHRASE.to_string(),
            playback_started: true,
            playback_error: None,
            user_mic_phrase: None,
            mic_prompt_started_at: None,
            mic_prompt_ended_at: None,
            mic_prompt_shown: None,
            mic_prompt_error: None,
            system_playback_started_at: None,
            system_playback_ended_at: None,
        },
    )
    .unwrap();

    let report = build_and_write_report(LiveTestReportInput {
        session_id: "session-1".to_string(),
        output_dir: output_dir.clone(),
        capture_dir,
        manifest_status: "done".to_string(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error: None,
        transcription_error: None,
    })
    .unwrap();

    assert!(report.passed());
    assert_eq!(report.schema_version, 9);
    assert_eq!(
        report.next_actions,
        vec!["No action needed. The live test passed."]
    );
    assert!(output_dir.join(REPORT_JSON_FILE).exists());
    assert!(output_dir.join(REPORT_MARKDOWN_FILE).exists());
}

#[test]
fn report_fails_when_audio_duration_exceeds_capture_wall_clock() {
    let dir = tempdir().unwrap();
    let output_dir = dir.path().join("recording");
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&capture_dir).unwrap();
    write_stereo_wav(&output_dir.join("source.wav"), 17, 0.25, 0.35);
    poha_mp3::encode_wav(
        &output_dir.join("source.wav"),
        &output_dir.join("audio.mp3"),
    )
    .unwrap();
    write_mono_wav(&output_dir.join("audio_mic.wav"), 17, 0.25);
    write_mono_wav(&output_dir.join("audio_mic_processed.wav"), 17, 0.22);
    write_mono_wav(&output_dir.join("audio_spk.wav"), 17, 0.35);
    write_transcript(
        &output_dir,
        "Poha live test recording microphone and system audio.",
    );
    let stopped_at = Utc::now();
    write_session_json(&output_dir, stopped_at - Duration::seconds(9));
    write_runtime_note(
        &output_dir,
        &LiveTestRuntimeNote {
            mode: LiveTestMode::Automatic,
            generated_at: stopped_at.to_rfc3339(),
            warmup: CaptureWarmup {
                ready: true,
                waited_ms: 500,
                observed_files: vec![CaptureFileProbe {
                    path: path_string(&capture_dir.join("audio.wav")),
                    exists: true,
                    bytes: Some(MIN_CAPTURE_BYTES),
                }],
            },
            diagnostic_phrase: DIAGNOSTIC_PHRASE.to_string(),
            playback_started: true,
            playback_error: None,
            user_mic_phrase: None,
            mic_prompt_started_at: None,
            mic_prompt_ended_at: None,
            mic_prompt_shown: None,
            mic_prompt_error: None,
            system_playback_started_at: None,
            system_playback_ended_at: None,
        },
    )
    .unwrap();

    let report = build_and_write_report(LiveTestReportInput {
        session_id: "session-1".to_string(),
        output_dir,
        capture_dir,
        manifest_status: "done".to_string(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error: None,
        transcription_error: None,
    })
    .unwrap();

    assert!(!report.passed());
    assert!(report.failure_summary().contains("captureTimebase"));
    assert!(
        report
            .next_actions
            .iter()
            .any(|action| action.contains("sample-rate"))
    );
}

#[test]
fn guided_report_checks_mic_system_phases_and_phrases() {
    let dir = tempdir().unwrap();
    let output_dir = dir.path().join("recording");
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&capture_dir).unwrap();
    write_stereo_wav(&output_dir.join("source.wav"), 14, 0.25, 0.35);
    poha_mp3::encode_wav(
        &output_dir.join("source.wav"),
        &output_dir.join("audio.mp3"),
    )
    .unwrap();
    write_mono_wav(&output_dir.join("audio_mic.wav"), 14, 0.25);
    write_mono_wav(&output_dir.join("audio_mic_processed.wav"), 14, 0.22);
    write_mono_wav(&output_dir.join("audio_spk.wav"), 14, 0.35);
    write_transcript(
        &output_dir,
        "oh my microphone check I am doing check on the boa microphone. Poha system audio check.",
    );
    let started_at = Utc::now();
    let stopped_at = started_at + Duration::seconds(15);
    write_session_json(&output_dir, started_at);
    write_runtime_note(
        &output_dir,
        &LiveTestRuntimeNote {
            mode: LiveTestMode::Guided,
            generated_at: stopped_at.to_rfc3339(),
            warmup: CaptureWarmup {
                ready: true,
                waited_ms: 500,
                observed_files: vec![CaptureFileProbe {
                    path: path_string(&capture_dir.join("audio.wav")),
                    exists: true,
                    bytes: Some(MIN_CAPTURE_BYTES),
                }],
            },
            diagnostic_phrase: GUIDED_SYSTEM_PHRASE.to_string(),
            playback_started: true,
            playback_error: None,
            user_mic_phrase: Some(GUIDED_MIC_PHRASE.to_string()),
            mic_prompt_started_at: Some((started_at + Duration::seconds(1)).to_rfc3339()),
            mic_prompt_ended_at: Some((started_at + Duration::seconds(7)).to_rfc3339()),
            mic_prompt_shown: Some(true),
            mic_prompt_error: None,
            system_playback_started_at: Some((started_at + Duration::seconds(8)).to_rfc3339()),
            system_playback_ended_at: Some((started_at + Duration::seconds(13)).to_rfc3339()),
        },
    )
    .unwrap();

    let report = build_and_write_report(LiveTestReportInput {
        session_id: "session-1".to_string(),
        output_dir,
        capture_dir,
        manifest_status: "done".to_string(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error: None,
        transcription_error: None,
    })
    .unwrap();

    assert!(report.passed());
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "guidedMicAudio" && check.status == "passed")
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "guidedTranscriptPhrases" && check.status == "passed")
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "processedMicBleedReduction" && check.status == "warning")
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "guidedMixedMp3MicPhase" && check.status == "passed")
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| { check.name == "guidedMixedMp3SystemPhase" && check.status == "passed" })
    );
    assert!(
        report.checks.iter().any(|check| {
            check.name == "guidedMixedMp3SystemBleed" && check.status == "warning"
        })
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "guidedTranscriptTiming" && check.status == "warning")
    );
}

#[test]
fn guided_report_fails_when_user_mic_phase_is_too_quiet() {
    let dir = tempdir().unwrap();
    let output_dir = dir.path().join("recording");
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&capture_dir).unwrap();
    write_stereo_wav(&output_dir.join("source.wav"), 14, 0.0001, 0.35);
    poha_mp3::encode_wav(
        &output_dir.join("source.wav"),
        &output_dir.join("audio.mp3"),
    )
    .unwrap();
    write_mono_wav(&output_dir.join("audio_mic.wav"), 14, 0.0001);
    write_mono_wav(&output_dir.join("audio_mic_processed.wav"), 14, 0.0001);
    write_mono_wav(&output_dir.join("audio_spk.wav"), 14, 0.35);
    write_transcript(
        &output_dir,
        "Poha microphone check. Poha system audio check.",
    );
    let started_at = Utc::now();
    let stopped_at = started_at + Duration::seconds(15);
    write_session_json(&output_dir, started_at);
    write_runtime_note(
        &output_dir,
        &LiveTestRuntimeNote {
            mode: LiveTestMode::Guided,
            generated_at: stopped_at.to_rfc3339(),
            warmup: CaptureWarmup {
                ready: true,
                waited_ms: 500,
                observed_files: vec![CaptureFileProbe {
                    path: path_string(&capture_dir.join("audio.wav")),
                    exists: true,
                    bytes: Some(MIN_CAPTURE_BYTES),
                }],
            },
            diagnostic_phrase: GUIDED_SYSTEM_PHRASE.to_string(),
            playback_started: true,
            playback_error: None,
            user_mic_phrase: Some(GUIDED_MIC_PHRASE.to_string()),
            mic_prompt_started_at: Some((started_at + Duration::seconds(1)).to_rfc3339()),
            mic_prompt_ended_at: Some((started_at + Duration::seconds(7)).to_rfc3339()),
            mic_prompt_shown: Some(true),
            mic_prompt_error: None,
            system_playback_started_at: Some((started_at + Duration::seconds(8)).to_rfc3339()),
            system_playback_ended_at: Some((started_at + Duration::seconds(13)).to_rfc3339()),
        },
    )
    .unwrap();

    let report = build_and_write_report(LiveTestReportInput {
        session_id: "session-1".to_string(),
        output_dir,
        capture_dir,
        manifest_status: "done".to_string(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error: None,
        transcription_error: None,
    })
    .unwrap();

    assert!(!report.passed());
    assert!(report.failure_summary().contains("guidedMicAudio"));
    assert!(
        report
            .next_actions
            .iter()
            .any(|action| action.contains("Move closer"))
    );
}

#[test]
fn report_fails_for_empty_live_test_artifacts() {
    let dir = tempdir().unwrap();
    let output_dir = dir.path().join("recording");
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&capture_dir).unwrap();
    write_transcript(&output_dir, "");

    let report = build_and_write_report(LiveTestReportInput {
        session_id: "session-1".to_string(),
        output_dir,
        capture_dir,
        manifest_status: "done".to_string(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error: None,
        transcription_error: None,
    })
    .unwrap();

    assert!(!report.passed());
    assert!(report.failure_summary().contains("mixedAudioMp3"));
    assert!(
        report
            .next_actions
            .iter()
            .any(|action| action.contains("audio.mp3"))
    );
}

#[test]
fn permission_failure_report_fails_before_capture() {
    let dir = tempdir().unwrap();

    let report = write_permission_failure_report(dir.path(), false, true).unwrap();

    assert!(!report.passed());
    assert!(report.failure_summary().contains("permissions"));
    assert!(
        report
            .next_actions
            .iter()
            .any(|action| action.contains("System Settings"))
    );
    assert!(report.report_markdown_path().exists());
}

#[tokio::test]
async fn capture_warmup_times_out_without_frames() {
    let dir = tempdir().unwrap();

    let warmup = wait_for_capture_audio(
        dir.path().to_path_buf(),
        std::time::Duration::from_millis(10),
    )
    .await;

    assert!(!warmup.ready);
    assert!(warmup.waited_ms < 1_000);
}

fn write_transcript(output_dir: &Path, text: &str) {
    let transcript = json!({
        "segments": if text.is_empty() {
            Vec::<Value>::new()
        } else {
            vec![json!({"startMs": 0, "endMs": 1_000, "text": text})]
        }
    });
    std::fs::write(
        output_dir.join("transcript.json"),
        serde_json::to_string_pretty(&transcript).unwrap(),
    )
    .unwrap();
}

fn write_session_json(output_dir: &Path, started_at: chrono::DateTime<Utc>) {
    let session = json!({
        "startedAt": started_at.to_rfc3339(),
    });
    std::fs::write(
        output_dir.join("session.json"),
        serde_json::to_string_pretty(&session).unwrap(),
    )
    .unwrap();
}

fn write_stereo_wav(path: &Path, seconds: u32, mic: f32, speaker: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 16_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..(seconds * 16_000) {
        writer.write_sample(mic).unwrap();
        writer.write_sample(speaker).unwrap();
    }
    writer.finalize().unwrap();
}

fn write_mono_wav(path: &Path, seconds: u32, value: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..(seconds * 16_000) {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}
