use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::stats::{
    AudioAnalysisWindow, AudioStats, audio_stats, audio_stats_for_window, storage_lifecycle_stats,
    transcript_stats,
};
use super::{
    GUIDED_MIC_PHRASE, GUIDED_SYSTEM_PHRASE, LiveTestMode, LiveTestRuntimeNote, REPORT_JSON_FILE,
    REPORT_MARKDOWN_FILE, RUNTIME_NOTE_FILE, read_runtime_note,
};

const MAX_CAPTURE_TIMEBASE_DRIFT_MS: u64 = 3_000;
const MIN_GUIDED_PHASE_AUDIO_MS: u64 = 1_000;
const MIN_GUIDED_MIC_RMS_DBFS: f64 = -55.0;
const MIN_GUIDED_SYSTEM_RMS_DBFS: f64 = -55.0;
const MAX_GUIDED_MIC_BELOW_SYSTEM_DB: f64 = 30.0;
const MAX_FINAL_MP3_MIC_BELOW_SYSTEM_DB: f64 = 10.0;
const MIN_FINAL_MP3_SYSTEM_BLEED_RMS_DBFS: f64 = -55.0;
const MIN_FINAL_MP3_SYSTEM_BLEED_SEPARATION_DB: f64 = 12.0;
const MAX_GUIDED_TRANSCRIPT_TIMING_DRIFT_MS: u64 = 3_000;
const MIN_SYSTEM_BLEED_RMS_DBFS: f64 = -55.0;
const MIN_PROCESSED_MIC_BLEED_REDUCTION_DB: f64 = 6.0;

#[derive(Debug, Clone)]
pub struct LiveTestReportInput {
    pub session_id: String,
    pub output_dir: PathBuf,
    pub capture_dir: PathBuf,
    pub manifest_status: String,
    pub microphone_permission: bool,
    pub system_audio_permission: bool,
    pub capture_error: Option<String>,
    pub transcription_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTestReport {
    pub schema_version: u32,
    pub session_id: String,
    pub status: String,
    pub generated_at: String,
    pub summary: String,
    pub next_actions: Vec<String>,
    pub paths: LiveTestPaths,
    pub checks: Vec<LiveTestCheck>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTestPaths {
    pub output_dir: String,
    pub capture_dir: String,
    pub report_json_path: String,
    pub report_markdown_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTestCheck {
    pub name: String,
    pub status: String,
    pub detail: String,
    pub observed: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTiming {
    started_at: String,
}

struct TimebaseCheck {
    status: &'static str,
    detail: &'static str,
    observed: Value,
}

impl LiveTestReport {
    pub fn passed(&self) -> bool {
        self.status == "passed"
    }

    pub fn report_markdown_path(&self) -> PathBuf {
        PathBuf::from(&self.paths.report_markdown_path)
    }

    pub fn failure_summary(&self) -> String {
        self.checks
            .iter()
            .filter(|check| check.status == "failed")
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn build_and_write_report(input: LiveTestReportInput) -> Result<LiveTestReport, String> {
    std::fs::create_dir_all(&input.output_dir)
        .map_err(|error| format!("failed creating live test output dir: {error}"))?;
    let report = build_report(input);
    let json_path = PathBuf::from(&report.paths.report_json_path);
    let markdown_path = PathBuf::from(&report.paths.report_markdown_path);
    let json = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("failed serializing live test report: {error}"))?;
    std::fs::write(&json_path, json)
        .map_err(|error| format!("failed writing {}: {error}", json_path.display()))?;
    std::fs::write(&markdown_path, report_markdown(&report))
        .map_err(|error| format!("failed writing {}: {error}", markdown_path.display()))?;
    Ok(report)
}

fn build_report(input: LiveTestReportInput) -> LiveTestReport {
    let runtime_note = read_runtime_note(&input.output_dir);
    let mixed_mp3_path = input.output_dir.join("audio.mp3");
    let mixed_stats = audio_stats(&mixed_mp3_path);
    let mic_stats = audio_stats(&input.output_dir.join("audio_mic.wav"));
    let processed_mic_stats = audio_stats(&input.output_dir.join("audio_mic_processed.wav"));
    let speaker_stats = audio_stats(&input.output_dir.join("audio_spk.wav"));
    let transcript_stats = transcript_stats(&input.output_dir.join("transcript.json"));
    let storage_stats =
        storage_lifecycle_stats(&input.output_dir, &input.capture_dir, &mixed_stats);
    let guided_mic_window = runtime_note
        .as_ref()
        .and_then(|note| guided_phase_window(&input.output_dir, note, GuidedPhase::Mic));
    let guided_system_window = runtime_note
        .as_ref()
        .and_then(|note| guided_phase_window(&input.output_dir, note, GuidedPhase::System));
    let guided_mic_stats = guided_mic_window.map(|window| {
        audio_stats_for_window(&input.output_dir.join("audio_mic.wav"), Some(window))
    });
    let guided_system_stats = guided_system_window.map(|window| {
        audio_stats_for_window(&input.output_dir.join("audio_spk.wav"), Some(window))
    });
    let guided_raw_mic_system_stats = guided_system_window.map(|window| {
        audio_stats_for_window(&input.output_dir.join("audio_mic.wav"), Some(window))
    });
    let guided_processed_mic_system_stats = guided_system_window.map(|window| {
        audio_stats_for_window(
            &input.output_dir.join("audio_mic_processed.wav"),
            Some(window),
        )
    });
    let guided_mixed_mic_stats =
        guided_mic_window.map(|window| audio_stats_for_window(&mixed_mp3_path, Some(window)));
    let guided_mixed_system_stats =
        guided_system_window.map(|window| audio_stats_for_window(&mixed_mp3_path, Some(window)));
    let permissions_ok = input.microphone_permission && input.system_audio_permission;
    let mut checks = Vec::new();

    push_check(
        &mut checks,
        "permissions",
        status(permissions_ok),
        if permissions_ok {
            "microphone and system audio permissions were authorized before capture started"
        } else {
            "one or more required capture permissions are missing"
        },
        json!({
            "microphone": permission_value(input.microphone_permission),
            "systemAudio": permission_value(input.system_audio_permission),
        }),
    );

    push_check(
        &mut checks,
        "captureWarmup",
        dependent_status(
            permissions_ok,
            runtime_note.as_ref().is_some_and(|note| note.warmup.ready),
        ),
        dependent_detail(
            permissions_ok,
            runtime_note.as_ref().map(|note| note.warmup.ready),
            "capture files received audio frames before the timeout",
            "capture files did not receive audio frames before the timeout",
            "live test runtime note was not written",
        ),
        runtime_note
            .as_ref()
            .map(|note| json!(note.warmup))
            .unwrap_or_else(
                || json!({"path": input.output_dir.join(RUNTIME_NOTE_FILE).to_string_lossy()}),
            ),
    );

    push_check(
        &mut checks,
        "diagnosticPlayback",
        dependent_status(
            permissions_ok,
            runtime_note
                .as_ref()
                .is_some_and(|note| note.playback_started),
        ),
        dependent_detail(
            permissions_ok,
            runtime_note.as_ref().map(|note| note.playback_started),
            "diagnostic phrase playback started",
            "diagnostic phrase playback did not start",
            "playback state was unavailable because the runtime note was missing",
        ),
        runtime_note
            .as_ref()
            .map(|note| {
                json!({
                    "phrase": note.diagnostic_phrase,
                    "error": note.playback_error,
                })
            })
            .unwrap_or_else(|| json!({})),
    );

    push_check(
        &mut checks,
        "captureLifecycle",
        dependent_status(
            permissions_ok,
            input.capture_error.is_none() && input.manifest_status == "done",
        ),
        if permissions_ok {
            "capture stopped, finalized, and manifest reached done status"
        } else {
            "skipped because capture permissions are missing"
        },
        json!({
            "manifestStatus": input.manifest_status,
            "captureError": input.capture_error,
            "transcriptionError": input.transcription_error,
        }),
    );

    push_check(
        &mut checks,
        "mixedAudioMp3",
        dependent_status(permissions_ok, mixed_stats.mixed_mp3_passed()),
        if permissions_ok {
            "final mixed MP3 exists, is stereo, decodes, and has sufficient duration"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(mixed_stats),
    );

    let mixed_mp3_balance = mixed_mp3_balance_delta(&mixed_stats);
    push_check(
        &mut checks,
        "mixedMp3AudioBalance",
        mixed_mp3_balance_status(permissions_ok, &mixed_mp3_balance),
        mixed_mp3_balance_detail(permissions_ok, &mixed_mp3_balance),
        json!({
            "micChannelRmsDbfs": mixed_stats.channel_rms_dbfs(0),
            "systemChannelRmsDbfs": mixed_stats.channel_rms_dbfs(1),
            "micMinusSystemDb": mixed_mp3_balance,
            "maxMicBelowSystemDb": MAX_FINAL_MP3_MIC_BELOW_SYSTEM_DB,
        }),
    );

    push_check(
        &mut checks,
        "microphoneStem",
        dependent_status(permissions_ok, mic_stats.stem_passed()),
        if permissions_ok {
            "pre-maintenance raw microphone stem existed, decoded, and was audible"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(mic_stats),
    );

    push_check(
        &mut checks,
        "processedMicrophoneStem",
        dependent_status(permissions_ok, processed_mic_stats.stem_passed()),
        if permissions_ok {
            "pre-maintenance processed microphone stem existed, decoded, and was audible"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(processed_mic_stats),
    );

    push_check(
        &mut checks,
        "systemAudioStem",
        dependent_status(permissions_ok, speaker_stats.stem_passed()),
        if permissions_ok {
            "pre-maintenance system audio stem existed, decoded, and was audible"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(speaker_stats),
    );

    if runtime_note
        .as_ref()
        .is_some_and(|note| note.mode == LiveTestMode::Guided)
    {
        push_guided_checks(
            &mut checks,
            permissions_ok,
            runtime_note.as_ref(),
            guided_system_window,
            guided_mic_stats.as_ref(),
            guided_raw_mic_system_stats.as_ref(),
            guided_processed_mic_system_stats.as_ref(),
            guided_system_stats.as_ref(),
            guided_mixed_mic_stats.as_ref(),
            guided_mixed_system_stats.as_ref(),
            &transcript_stats,
        );
    }

    let timebase = capture_timebase(&input.output_dir, runtime_note.as_ref(), &mixed_stats);
    push_check(
        &mut checks,
        "captureTimebase",
        if permissions_ok {
            timebase.status
        } else {
            "skipped"
        },
        if !permissions_ok {
            "skipped because capture permissions are missing"
        } else {
            timebase.detail
        },
        timebase.observed,
    );

    push_check(
        &mut checks,
        "transcript",
        dependent_status(
            permissions_ok,
            input.transcription_error.is_none() && transcript_stats.has_segments(),
        ),
        if permissions_ok {
            "transcript JSON exists and contains at least one segment"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(transcript_stats),
    );

    push_check(
        &mut checks,
        "storageLifecycle",
        dependent_status(permissions_ok, storage_stats.passed()),
        if permissions_ok {
            "final artifacts are compatible with default storage maintenance"
        } else {
            "skipped because capture permissions are missing"
        },
        json!(storage_stats),
    );

    let failed_count = checks
        .iter()
        .filter(|check| check.status == "failed")
        .count();
    let warning_count = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let status = if failed_count == 0 {
        "passed"
    } else {
        "failed"
    };

    LiveTestReport {
        schema_version: 9,
        session_id: input.session_id,
        status: status.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        summary: if failed_count == 0 && warning_count == 0 {
            "All live test checks passed.".to_string()
        } else if failed_count == 0 {
            format!("All blocking live test checks passed; {warning_count} warning(s).")
        } else {
            format!("{failed_count} live test check(s) failed.")
        },
        next_actions: next_actions_for(&checks),
        paths: LiveTestPaths {
            output_dir: path_string(&input.output_dir),
            capture_dir: path_string(&input.capture_dir),
            report_json_path: path_string(&input.output_dir.join(REPORT_JSON_FILE)),
            report_markdown_path: path_string(&input.output_dir.join(REPORT_MARKDOWN_FILE)),
        },
        checks,
    }
}

fn push_check(
    checks: &mut Vec<LiveTestCheck>,
    name: &str,
    status: &str,
    detail: &str,
    observed: Value,
) {
    checks.push(LiveTestCheck {
        name: name.to_string(),
        status: status.to_string(),
        detail: detail.to_string(),
        observed,
    });
}

fn mixed_mp3_balance_delta(mixed_stats: &AudioStats) -> Option<f64> {
    Some(round_db(
        mixed_stats.channel_rms_dbfs(0)? - mixed_stats.channel_rms_dbfs(1)?,
    ))
}

fn mixed_mp3_balance_status(permissions_ok: bool, delta_db: &Option<f64>) -> &'static str {
    if !permissions_ok {
        return "skipped";
    }

    match delta_db {
        Some(delta_db) if *delta_db >= -MAX_FINAL_MP3_MIC_BELOW_SYSTEM_DB => "passed",
        Some(_) => "warning",
        None => "skipped",
    }
}

fn mixed_mp3_balance_detail(permissions_ok: bool, delta_db: &Option<f64>) -> &'static str {
    if !permissions_ok {
        "skipped because capture permissions are missing"
    } else if delta_db.is_none() {
        "skipped because final MP3 channel levels were unavailable"
    } else if delta_db.is_some_and(|delta| delta >= -MAX_FINAL_MP3_MIC_BELOW_SYSTEM_DB) {
        "final MP3 mic channel is balanced enough against system audio"
    } else {
        "final MP3 mic channel is much quieter than system audio"
    }
}

enum GuidedPhase {
    Mic,
    System,
}

fn push_guided_checks(
    checks: &mut Vec<LiveTestCheck>,
    permissions_ok: bool,
    runtime_note: Option<&LiveTestRuntimeNote>,
    guided_system_window: Option<AudioAnalysisWindow>,
    guided_mic_stats: Option<&AudioStats>,
    guided_raw_mic_system_stats: Option<&AudioStats>,
    guided_processed_mic_system_stats: Option<&AudioStats>,
    guided_system_stats: Option<&AudioStats>,
    guided_mixed_mic_stats: Option<&AudioStats>,
    guided_mixed_system_stats: Option<&AudioStats>,
    transcript_stats: &super::stats::TranscriptStats,
) {
    let mic_prompt_ready = runtime_note.is_some_and(|note| {
        note.user_mic_phrase.as_deref() == Some(GUIDED_MIC_PHRASE)
            && note.mic_prompt_started_at.is_some()
            && note.mic_prompt_ended_at.is_some()
    });
    push_check(
        checks,
        "guidedMicPrompt",
        dependent_status(permissions_ok, mic_prompt_ready),
        if permissions_ok {
            "guided test prompted the user to speak the microphone phrase"
        } else {
            "skipped because capture permissions are missing"
        },
        runtime_note
            .map(|note| {
                json!({
                    "phrase": note.user_mic_phrase,
                    "promptShown": note.mic_prompt_shown,
                    "promptError": note.mic_prompt_error,
                    "startedAt": note.mic_prompt_started_at,
                    "endedAt": note.mic_prompt_ended_at,
                })
            })
            .unwrap_or_else(|| json!({})),
    );

    let mic_level_ready = guided_mic_stats.is_some_and(|stats| {
        stats.audible_window_passed(MIN_GUIDED_PHASE_AUDIO_MS, MIN_GUIDED_MIC_RMS_DBFS)
    });
    push_check(
        checks,
        "guidedMicAudio",
        dependent_status(permissions_ok, mic_level_ready),
        if permissions_ok {
            "pre-maintenance microphone stem is audible during the user speech phase"
        } else {
            "skipped because capture permissions are missing"
        },
        guided_mic_stats
            .map(|stats| {
                json!({
                    "thresholds": {
                        "minAnalyzedMs": MIN_GUIDED_PHASE_AUDIO_MS,
                        "minRmsDbfs": MIN_GUIDED_MIC_RMS_DBFS,
                    },
                    "stats": stats,
                })
            })
            .unwrap_or_else(|| json!({"error": "guided microphone phase window unavailable"})),
    );

    let system_level_ready = guided_system_stats.is_some_and(|stats| {
        stats.audible_window_passed(MIN_GUIDED_PHASE_AUDIO_MS, MIN_GUIDED_SYSTEM_RMS_DBFS)
    });
    push_check(
        checks,
        "guidedSystemAudio",
        dependent_status(permissions_ok, system_level_ready),
        if permissions_ok {
            "pre-maintenance system stem is audible during the system playback phase"
        } else {
            "skipped because capture permissions are missing"
        },
        guided_system_stats
            .map(|stats| {
                json!({
                    "phrase": GUIDED_SYSTEM_PHRASE,
                    "thresholds": {
                        "minAnalyzedMs": MIN_GUIDED_PHASE_AUDIO_MS,
                        "minRmsDbfs": MIN_GUIDED_SYSTEM_RMS_DBFS,
                    },
                    "stats": stats,
                })
            })
            .unwrap_or_else(|| json!({"error": "guided system phase window unavailable"})),
    );

    push_guided_mixed_mp3_checks(
        checks,
        permissions_ok,
        guided_mixed_mic_stats,
        guided_mixed_system_stats,
    );

    let bleed_reduction = processed_mic_bleed_reduction(
        guided_raw_mic_system_stats,
        guided_processed_mic_system_stats,
    );
    push_check(
        checks,
        "processedMicBleedReduction",
        processed_mic_bleed_status(
            permissions_ok,
            guided_raw_mic_system_stats,
            &bleed_reduction,
        ),
        processed_mic_bleed_detail(
            permissions_ok,
            guided_raw_mic_system_stats,
            &bleed_reduction,
        ),
        json!({
            "rawMicSystemPhaseRmsDbfs": guided_raw_mic_system_stats.and_then(AudioStats::rms_dbfs),
            "processedMicSystemPhaseRmsDbfs": guided_processed_mic_system_stats.and_then(AudioStats::rms_dbfs),
            "systemRmsDbfs": guided_system_stats.and_then(AudioStats::rms_dbfs),
            "rawBleedThresholdDbfs": MIN_SYSTEM_BLEED_RMS_DBFS,
            "minReductionDb": MIN_PROCESSED_MIC_BLEED_REDUCTION_DB,
            "observedReductionDb": bleed_reduction,
        }),
    );

    let balance_ready = guided_level_balance_passed(guided_mic_stats, guided_system_stats);
    push_check(
        checks,
        "guidedAudioBalance",
        dependent_status(permissions_ok, balance_ready),
        if permissions_ok {
            "user mic level is not excessively lower than system playback level"
        } else {
            "skipped because capture permissions are missing"
        },
        json!({
            "micRmsDbfs": guided_mic_stats.and_then(AudioStats::rms_dbfs),
            "systemRmsDbfs": guided_system_stats.and_then(AudioStats::rms_dbfs),
            "maxMicBelowSystemDb": MAX_GUIDED_MIC_BELOW_SYSTEM_DB,
        }),
    );

    let transcript_has_mic_phrase = guided_mic_transcript_passed(transcript_stats);
    let transcript_has_system_phrase = transcript_stats.contains_phrase(GUIDED_SYSTEM_PHRASE);
    let transcript_has_phrases = transcript_has_mic_phrase && transcript_has_system_phrase;
    push_check(
        checks,
        "guidedTranscriptPhrases",
        dependent_status(
            permissions_ok,
            transcript_stats.has_segments() && transcript_has_phrases,
        ),
        if permissions_ok {
            "transcript contains the guided microphone phrase content and system phrase"
        } else {
            "skipped because capture permissions are missing"
        },
        json!({
            "micPhrase": GUIDED_MIC_PHRASE,
            "systemPhrase": GUIDED_SYSTEM_PHRASE,
            "containsMicPhrase": transcript_stats.contains_phrase(GUIDED_MIC_PHRASE),
            "containsMicCorePhrase": transcript_stats.contains_phrase("microphone check"),
            "containsSystemPhrase": transcript_has_system_phrase,
        }),
    );

    let transcript_timing = guided_transcript_timing(guided_system_window, transcript_stats);
    push_check(
        checks,
        "guidedTranscriptTiming",
        guided_transcript_timing_status(
            permissions_ok,
            transcript_has_system_phrase,
            &transcript_timing,
        ),
        guided_transcript_timing_detail(
            permissions_ok,
            transcript_has_system_phrase,
            &transcript_timing,
        ),
        json!({
            "systemPhrase": GUIDED_SYSTEM_PHRASE,
            "expectedStartMs": guided_system_window.map(|window| window.start_ms),
            "transcriptStartMs": transcript_timing.map(|timing| timing.transcript_start_ms),
            "deltaMs": transcript_timing.map(|timing| timing.delta_ms),
            "maxDeltaMs": MAX_GUIDED_TRANSCRIPT_TIMING_DRIFT_MS,
        }),
    );
}

fn push_guided_mixed_mp3_checks(
    checks: &mut Vec<LiveTestCheck>,
    permissions_ok: bool,
    guided_mixed_mic_stats: Option<&AudioStats>,
    guided_mixed_system_stats: Option<&AudioStats>,
) {
    let mic_phase_ready = guided_mixed_mic_stats.is_some_and(|stats| {
        stats.channel_audible_window_passed(0, MIN_GUIDED_PHASE_AUDIO_MS, MIN_GUIDED_MIC_RMS_DBFS)
    });
    push_check(
        checks,
        "guidedMixedMp3MicPhase",
        dependent_status(permissions_ok, mic_phase_ready),
        if permissions_ok {
            "final MP3 mic channel is audible during the user speech phase"
        } else {
            "skipped because capture permissions are missing"
        },
        guided_mixed_mic_stats
            .map(|stats| {
                json!({
                    "channel": "left",
                    "thresholds": {
                        "minAnalyzedMs": MIN_GUIDED_PHASE_AUDIO_MS,
                        "minRmsDbfs": MIN_GUIDED_MIC_RMS_DBFS,
                    },
                    "micChannelRmsDbfs": stats.channel_rms_dbfs(0),
                    "systemChannelRmsDbfs": stats.channel_rms_dbfs(1),
                    "stats": stats,
                })
            })
            .unwrap_or_else(
                || json!({"error": "guided final MP3 microphone phase window unavailable"}),
            ),
    );

    let system_phase_ready = guided_mixed_system_stats.is_some_and(|stats| {
        stats.channel_audible_window_passed(
            1,
            MIN_GUIDED_PHASE_AUDIO_MS,
            MIN_GUIDED_SYSTEM_RMS_DBFS,
        )
    });
    push_check(
        checks,
        "guidedMixedMp3SystemPhase",
        dependent_status(permissions_ok, system_phase_ready),
        if permissions_ok {
            "final MP3 system channel is audible during the system playback phase"
        } else {
            "skipped because capture permissions are missing"
        },
        guided_mixed_system_stats
            .map(|stats| {
                json!({
                    "channel": "right",
                    "thresholds": {
                        "minAnalyzedMs": MIN_GUIDED_PHASE_AUDIO_MS,
                        "minRmsDbfs": MIN_GUIDED_SYSTEM_RMS_DBFS,
                    },
                    "micChannelRmsDbfs": stats.channel_rms_dbfs(0),
                    "systemChannelRmsDbfs": stats.channel_rms_dbfs(1),
                    "stats": stats,
                })
            })
            .unwrap_or_else(
                || json!({"error": "guided final MP3 system phase window unavailable"}),
            ),
    );

    let bleed_separation = final_mp3_system_bleed_separation(guided_mixed_system_stats);
    push_check(
        checks,
        "guidedMixedMp3SystemBleed",
        final_mp3_system_bleed_status(permissions_ok, guided_mixed_system_stats, &bleed_separation),
        final_mp3_system_bleed_detail(permissions_ok, guided_mixed_system_stats, &bleed_separation),
        json!({
            "micChannelRmsDbfs": guided_mixed_system_stats.and_then(|stats| stats.channel_rms_dbfs(0)),
            "systemChannelRmsDbfs": guided_mixed_system_stats.and_then(|stats| stats.channel_rms_dbfs(1)),
            "systemMinusMicDb": bleed_separation,
            "quietBleedThresholdDbfs": MIN_FINAL_MP3_SYSTEM_BLEED_RMS_DBFS,
            "minSystemAboveMicDb": MIN_FINAL_MP3_SYSTEM_BLEED_SEPARATION_DB,
        }),
    );
}

#[derive(Debug, Clone, Copy)]
struct GuidedTranscriptTiming {
    transcript_start_ms: i64,
    delta_ms: u64,
}

fn guided_mic_transcript_passed(transcript_stats: &super::stats::TranscriptStats) -> bool {
    transcript_stats.contains_phrase(GUIDED_MIC_PHRASE)
        || transcript_stats.contains_phrase("microphone check")
}

fn guided_transcript_timing(
    guided_system_window: Option<AudioAnalysisWindow>,
    transcript_stats: &super::stats::TranscriptStats,
) -> Option<GuidedTranscriptTiming> {
    let expected_start_ms = i64::try_from(guided_system_window?.start_ms).ok()?;
    let transcript_start_ms = transcript_stats.phrase_start_ms(GUIDED_SYSTEM_PHRASE)?;
    Some(GuidedTranscriptTiming {
        transcript_start_ms,
        delta_ms: transcript_start_ms.abs_diff(expected_start_ms),
    })
}

fn guided_transcript_timing_status(
    permissions_ok: bool,
    transcript_has_system_phrase: bool,
    timing: &Option<GuidedTranscriptTiming>,
) -> &'static str {
    if !permissions_ok {
        return "skipped";
    }
    if !transcript_has_system_phrase || timing.is_none() {
        return "skipped";
    }
    if timing.is_some_and(|timing| timing.delta_ms <= MAX_GUIDED_TRANSCRIPT_TIMING_DRIFT_MS) {
        "passed"
    } else {
        "warning"
    }
}

fn guided_transcript_timing_detail(
    permissions_ok: bool,
    transcript_has_system_phrase: bool,
    timing: &Option<GuidedTranscriptTiming>,
) -> &'static str {
    if !permissions_ok {
        "skipped because capture permissions are missing"
    } else if !transcript_has_system_phrase {
        "skipped because the guided system phrase was not found in transcript"
    } else if timing.is_none() {
        "skipped because transcript or guided system timing was unavailable"
    } else if timing.is_some_and(|timing| timing.delta_ms <= MAX_GUIDED_TRANSCRIPT_TIMING_DRIFT_MS)
    {
        "guided system transcript timing matches playback timing"
    } else {
        "guided system transcript timing is far from playback timing"
    }
}

fn processed_mic_bleed_reduction(
    raw_mic_system_stats: Option<&AudioStats>,
    processed_mic_system_stats: Option<&AudioStats>,
) -> Option<f64> {
    Some(round_db(
        raw_mic_system_stats.and_then(AudioStats::rms_dbfs)?
            - processed_mic_system_stats.and_then(AudioStats::rms_dbfs)?,
    ))
}

fn processed_mic_bleed_status(
    permissions_ok: bool,
    raw_mic_system_stats: Option<&AudioStats>,
    reduction_db: &Option<f64>,
) -> &'static str {
    if !permissions_ok {
        return "skipped";
    }

    if raw_mic_system_stats
        .and_then(AudioStats::rms_dbfs)
        .is_some_and(|value| value < MIN_SYSTEM_BLEED_RMS_DBFS)
    {
        return "passed";
    }

    let Some(reduction_db) = reduction_db else {
        return "skipped";
    };

    if *reduction_db >= MIN_PROCESSED_MIC_BLEED_REDUCTION_DB {
        "passed"
    } else {
        "warning"
    }
}

fn processed_mic_bleed_detail(
    permissions_ok: bool,
    raw_mic_system_stats: Option<&AudioStats>,
    reduction_db: &Option<f64>,
) -> &'static str {
    if !permissions_ok {
        "skipped because capture permissions are missing"
    } else if raw_mic_system_stats
        .and_then(AudioStats::rms_dbfs)
        .is_some_and(|value| value < MIN_SYSTEM_BLEED_RMS_DBFS)
    {
        "raw microphone stem did not capture meaningful system bleed during playback"
    } else if reduction_db.is_none() {
        "skipped because raw or processed mic system-phase levels were unavailable"
    } else if reduction_db.is_some_and(|value| value >= MIN_PROCESSED_MIC_BLEED_REDUCTION_DB) {
        "processed microphone stem reduces system bleed during playback"
    } else {
        "processed microphone stem did not materially reduce system bleed during playback"
    }
}

fn final_mp3_system_bleed_separation(system_phase_stats: Option<&AudioStats>) -> Option<f64> {
    Some(round_db(
        system_phase_stats?.channel_rms_dbfs(1)? - system_phase_stats?.channel_rms_dbfs(0)?,
    ))
}

fn final_mp3_system_bleed_status(
    permissions_ok: bool,
    system_phase_stats: Option<&AudioStats>,
    separation_db: &Option<f64>,
) -> &'static str {
    if !permissions_ok {
        return "skipped";
    }
    if system_phase_stats
        .and_then(|stats| stats.channel_rms_dbfs(0))
        .is_some_and(|value| value < MIN_FINAL_MP3_SYSTEM_BLEED_RMS_DBFS)
    {
        return "passed";
    }
    let Some(separation_db) = separation_db else {
        return "skipped";
    };
    if *separation_db >= MIN_FINAL_MP3_SYSTEM_BLEED_SEPARATION_DB {
        "passed"
    } else {
        "warning"
    }
}

fn final_mp3_system_bleed_detail(
    permissions_ok: bool,
    system_phase_stats: Option<&AudioStats>,
    separation_db: &Option<f64>,
) -> &'static str {
    if !permissions_ok {
        "skipped because capture permissions are missing"
    } else if system_phase_stats
        .and_then(|stats| stats.channel_rms_dbfs(0))
        .is_some_and(|value| value < MIN_FINAL_MP3_SYSTEM_BLEED_RMS_DBFS)
    {
        "final MP3 mic channel is quiet during system playback"
    } else if separation_db.is_none() {
        "skipped because final MP3 system-phase channel levels were unavailable"
    } else if separation_db.is_some_and(|value| value >= MIN_FINAL_MP3_SYSTEM_BLEED_SEPARATION_DB) {
        "final MP3 system channel is clearly above mic bleed during playback"
    } else {
        "final MP3 has noticeable mic-channel bleed during system playback"
    }
}

fn round_db(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn guided_level_balance_passed(
    guided_mic_stats: Option<&AudioStats>,
    guided_system_stats: Option<&AudioStats>,
) -> bool {
    let Some(mic_rms) = guided_mic_stats.and_then(AudioStats::rms_dbfs) else {
        return false;
    };
    let Some(system_rms) = guided_system_stats.and_then(AudioStats::rms_dbfs) else {
        return false;
    };
    mic_rms - system_rms >= -MAX_GUIDED_MIC_BELOW_SYSTEM_DB
}

fn guided_phase_window(
    output_dir: &Path,
    runtime_note: &LiveTestRuntimeNote,
    phase: GuidedPhase,
) -> Option<AudioAnalysisWindow> {
    if runtime_note.mode != LiveTestMode::Guided {
        return None;
    }
    let session_started_at = read_session_started_at(output_dir).ok()?;
    let (started_at, ended_at) = match phase {
        GuidedPhase::Mic => (
            runtime_note.mic_prompt_started_at.as_deref(),
            runtime_note.mic_prompt_ended_at.as_deref(),
        ),
        GuidedPhase::System => (
            runtime_note.system_playback_started_at.as_deref(),
            runtime_note.system_playback_ended_at.as_deref(),
        ),
    };
    let started_at = parse_rfc3339(started_at?).ok()?;
    let ended_at = parse_rfc3339(ended_at?).ok()?;
    let start_ms = started_at
        .signed_duration_since(session_started_at)
        .num_milliseconds()
        .max(0);
    let end_ms = ended_at
        .signed_duration_since(session_started_at)
        .num_milliseconds()
        .max(0);
    if end_ms <= start_ms {
        return None;
    }
    Some(AudioAnalysisWindow {
        start_ms: u64::try_from(start_ms).ok()?,
        end_ms: u64::try_from(end_ms).ok()?,
    })
}

fn capture_timebase(
    output_dir: &Path,
    runtime_note: Option<&LiveTestRuntimeNote>,
    mixed_stats: &AudioStats,
) -> TimebaseCheck {
    let Some(audio_duration_ms) = mixed_stats.duration_ms() else {
        return skipped_timebase("mixed audio duration was unavailable");
    };
    let Some(runtime_note) = runtime_note else {
        return skipped_timebase("live test runtime note was unavailable");
    };

    let started_at = match read_session_started_at(output_dir) {
        Ok(started_at) => started_at,
        Err(error) => return skipped_timebase(error),
    };
    let stopped_at = match parse_rfc3339(&runtime_note.generated_at) {
        Ok(stopped_at) => stopped_at,
        Err(error) => return skipped_timebase(error),
    };
    let capture_elapsed_ms = stopped_at
        .signed_duration_since(started_at)
        .num_milliseconds();
    if capture_elapsed_ms < 0 {
        return skipped_timebase("session start time is after live-test stop time");
    }

    let capture_elapsed_ms = u64::try_from(capture_elapsed_ms).unwrap_or(u64::MAX);
    let max_expected_duration_ms = capture_elapsed_ms.saturating_add(MAX_CAPTURE_TIMEBASE_DRIFT_MS);
    let passed = audio_duration_ms <= max_expected_duration_ms;

    TimebaseCheck {
        status: status(passed),
        detail: if passed {
            "mixed MP3 duration matches the live capture wall-clock window"
        } else {
            "mixed MP3 duration is longer than the live capture wall-clock window"
        },
        observed: json!({
            "audioDurationMs": audio_duration_ms,
            "captureElapsedMs": capture_elapsed_ms,
            "allowedDriftMs": MAX_CAPTURE_TIMEBASE_DRIFT_MS,
            "maxExpectedDurationMs": max_expected_duration_ms,
            "sessionStartedAt": started_at.to_rfc3339(),
            "runtimeGeneratedAt": stopped_at.to_rfc3339(),
        }),
    }
}

fn skipped_timebase(reason: impl Into<String>) -> TimebaseCheck {
    TimebaseCheck {
        status: "skipped",
        detail: "capture timebase could not be checked",
        observed: json!({ "reason": reason.into() }),
    }
}

fn read_session_started_at(output_dir: &Path) -> Result<DateTime<Utc>, String> {
    let path = output_dir.join("session.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    let timing: SessionTiming = serde_json::from_str(&content)
        .map_err(|error| format!("failed parsing session.json: {error}"))?;
    parse_rfc3339(&timing.started_at)
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|datetime| datetime.with_timezone(&Utc))
        .map_err(|error| format!("failed parsing timestamp {value:?}: {error}"))
}

fn report_markdown(report: &LiveTestReport) -> String {
    let mut markdown = format!(
        "# Poha Live Test Report\n\nStatus: {}\n\nSession: `{}`\n\n{}\n\n",
        report.status, report.session_id, report.summary
    );
    if !report.next_actions.is_empty() {
        markdown.push_str("## Next actions\n\n");
        for action in &report.next_actions {
            markdown.push_str(&format!("- {action}\n"));
        }
        markdown.push('\n');
    }
    markdown.push_str("## Checks\n\n");
    for check in &report.checks {
        markdown.push_str(&format!(
            "- {}: {} - {}\n",
            check.name, check.status, check.detail
        ));
    }
    markdown
}

fn dependent_detail(
    dependency_ok: bool,
    passed: Option<bool>,
    pass_detail: &'static str,
    fail_detail: &'static str,
    missing_detail: &'static str,
) -> &'static str {
    if !dependency_ok {
        "skipped because capture permissions are missing"
    } else if passed == Some(true) {
        pass_detail
    } else if passed == Some(false) {
        fail_detail
    } else {
        missing_detail
    }
}

fn permission_value(authorized: bool) -> &'static str {
    if authorized { "authorized" } else { "missing" }
}

fn status(passed: bool) -> &'static str {
    if passed { "passed" } else { "failed" }
}

fn dependent_status(dependency_ok: bool, passed: bool) -> &'static str {
    if dependency_ok {
        status(passed)
    } else {
        "skipped"
    }
}

fn next_actions_for(checks: &[LiveTestCheck]) -> Vec<String> {
    if checks.iter().all(|check| check.status != "failed") {
        return vec!["No action needed. The live test passed.".to_string()];
    }

    let failed = |name: &str| {
        checks
            .iter()
            .any(|check| check.name == name && check.status == "failed")
    };

    let mut actions = Vec::new();
    if failed("permissions") {
        actions.push(
            "Enable Poha under System Settings > Privacy & Security > Microphone, then rerun Run Live Test."
                .to_string(),
        );
    }
    if failed("captureWarmup") || failed("captureLifecycle") {
        actions.push(
            "Check the selected microphone and system audio sources, keep audible audio playing, then rerun Run Live Test."
                .to_string(),
        );
    }
    if failed("diagnosticPlayback") {
        actions.push(
            "Confirm the macOS say command can play audio, then rerun Run Live Test.".to_string(),
        );
    }
    if failed("mixedAudioMp3")
        || failed("guidedMixedMp3MicPhase")
        || failed("guidedMixedMp3SystemPhase")
        || failed("microphoneStem")
        || failed("processedMicrophoneStem")
        || failed("systemAudioStem")
    {
        actions.push(
            "Open the session folder and inspect audio.mp3, audio_mic.wav, audio_mic_processed.wav, and audio_spk.wav before storage cleanup."
                .to_string(),
        );
    }
    if failed("guidedMicPrompt") {
        actions.push(
            "Rerun Run Guided Audio Check and say the microphone phrase while the tray status prompts you."
                .to_string(),
        );
    }
    if failed("guidedMicAudio") || failed("guidedAudioBalance") {
        actions.push(
            "Move closer to the selected microphone or raise input gain, then rerun Run Guided Audio Check."
                .to_string(),
        );
    }
    if failed("guidedSystemAudio") {
        actions.push(
            "Raise system output volume and rerun Run Guided Audio Check without headphones if testing speaker capture."
                .to_string(),
        );
    }
    if failed("guidedTranscriptPhrases") {
        actions.push(
            "Check that both guided phrases are audible in audio.mp3, then inspect transcription logs."
                .to_string(),
        );
    }
    if failed("captureTimebase") {
        actions.push(
            "Inspect the live capture sample-rate and channel downmix path before trusting audio.mp3."
                .to_string(),
        );
    }
    if failed("transcript") {
        actions.push(
            "Check transcription provider or local model logs, then rerun transcription for the session."
                .to_string(),
        );
    }
    if failed("storageLifecycle") {
        actions.push(
            "Run poha-cli storage maintain --dry-run and inspect the reported artifact blockers."
                .to_string(),
        );
    }

    actions
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}
