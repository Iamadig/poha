use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use chrono::Utc;
use poha_audio_utils::{Source as _, mono_frames, source_from_path};
use serde::{Deserialize, Serialize};

use crate::recorder_settings::{RecorderSettings, SpeakerLabelMode};

const RAW_MIC_FILE: &str = "audio_mic.wav";
const PROCESSED_MIC_FILE: &str = "audio_mic_processed.wav";
const SYSTEM_AUDIO_FILE: &str = "audio_spk.wav";
const PROCESSED_MIC_DURATION_TOLERANCE_MS: u64 = 1_000;
const RAW_ONLY_MIC_ACTIVE_DBFS: f64 = -55.0;
const RAW_ONLY_SYSTEM_QUIET_DBFS: f64 = -62.0;
const PROCESSED_MIC_MIN_ACTIVE_DBFS: f64 = -65.0;
const MAX_RAW_ONLY_PROCESSED_ATTENUATION_DB: f64 = 24.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub speaker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    pub source: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationSegment {
    pub speaker_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptDocument {
    pub engine: String,
    pub model: String,
    pub fallback_model: String,
    pub speaker_label_mode: SpeakerLabelMode,
    pub generated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_inputs: Vec<TranscriptAudioInput>,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptAudioInput {
    pub speaker: String,
    pub source: String,
    pub file_name: String,
    pub path: String,
    pub selection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProcessInput {
    pub session_id: String,
    pub capture_dir: PathBuf,
    pub output_dir: PathBuf,
    pub audio_path_from_event: Option<PathBuf>,
    pub settings: RecorderSettings,
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub audio_output_path: Option<PathBuf>,
    pub transcript_json_path: PathBuf,
    pub transcript_markdown_path: PathBuf,
}

pub struct LiveTranscriptionHandle {
    stop_requested: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl LiveTranscriptionHandle {
    pub async fn stop(self) -> Result<(), String> {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.task
            .await
            .map_err(|error| format!("live transcription task join failed: {error}"))?
    }
}

pub fn spawn_live_transcription(input: ProcessInput) -> LiveTranscriptionHandle {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_requested);
    let task = tokio::task::spawn_blocking(move || run_live_transcription(input, worker_stop));

    LiveTranscriptionHandle {
        stop_requested,
        task,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkManifest {
    strategy: String,
    generated_at: String,
    chunks: Vec<ChunkManifestEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ChunkStatus {
    Queued,
    Transcribing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkManifestEntry {
    speaker: String,
    source: String,
    path: String,
    start_ms: i64,
    end_ms: i64,
    status: ChunkStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct AudioChunk {
    path: PathBuf,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveChunkSnapshot {
    strategy: String,
    generated_at: String,
    updated_at: String,
    chunks: Vec<LiveChunkEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveChunkEntry {
    speaker: String,
    source: String,
    path: String,
    start_ms: i64,
    end_ms: i64,
    status: ChunkStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ChunkKey {
    source: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug)]
enum PartialSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

#[derive(Debug)]
struct PartialWav {
    spec: hound::WavSpec,
    frames: Vec<f32>,
    samples: PartialSamples,
}

#[derive(Debug, Deserialize)]
struct MlxOutput {
    text: Option<String>,
    segments: Option<Vec<MlxSegment>>,
}

#[derive(Debug, Deserialize)]
struct MlxSegment {
    start: Option<f64>,
    end: Option<f64>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DiarizationPayload {
    Segments(Vec<DiarizationSegment>),
    Document { segments: Vec<DiarizationSegment> },
}

pub fn process_session(input: ProcessInput) -> Result<ProcessResult, String> {
    std::fs::create_dir_all(&input.output_dir).map_err(|e| {
        format!(
            "failed creating output dir {}: {e}",
            input.output_dir.display()
        )
    })?;

    copy_audio_artifacts(&input.capture_dir, &input.output_dir)?;
    let audio_output_path = first_existing(
        &input.output_dir,
        &["audio.mp3", "audio.wav", "audio.ogg", "audio.m4a"],
    );

    let diarization_segments = prepare_diarization_segments(&input.capture_dir, &input.output_dir)?;
    let (mut segments, audio_inputs) = build_segments(&input)?;
    apply_system_diarization(&mut segments, &diarization_segments);
    suppress_system_bleed_duplicates(&mut segments);

    let transcript = TranscriptDocument {
        engine: "mlx-whisper".to_string(),
        model: input.settings.mlx_model.clone(),
        fallback_model: input.settings.mlx_fallback_model.clone(),
        speaker_label_mode: input.settings.speaker_label_mode,
        generated_at: Utc::now().to_rfc3339(),
        audio_inputs,
        segments,
    };

    let transcript_json_path = input.output_dir.join("transcript.json");
    let transcript_markdown_path = input.output_dir.join("transcript.md");
    let transcript_json = serde_json::to_string_pretty(&transcript)
        .map_err(|e| format!("failed serializing transcript json: {e}"))?;
    std::fs::write(&transcript_json_path, transcript_json).map_err(|e| {
        format!(
            "failed writing transcript json {}: {e}",
            transcript_json_path.display()
        )
    })?;
    std::fs::write(
        &transcript_markdown_path,
        render_markdown(&transcript.segments),
    )
    .map_err(|e| {
        format!(
            "failed writing transcript markdown {}: {e}",
            transcript_markdown_path.display()
        )
    })?;

    Ok(ProcessResult {
        audio_output_path,
        transcript_json_path,
        transcript_markdown_path,
    })
}

fn copy_audio_artifacts(capture_dir: &Path, output_dir: &Path) -> Result<(), String> {
    for file_name in [
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        RAW_MIC_FILE,
        PROCESSED_MIC_FILE,
        SYSTEM_AUDIO_FILE,
    ] {
        let src = capture_dir.join(file_name);
        if !src.exists() {
            continue;
        }
        let dst = output_dir.join(file_name);
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("failed copying {} -> {}: {e}", src.display(), dst.display()))?;
    }
    Ok(())
}

fn read_diarization_segments(
    capture_dir: &Path,
    output_dir: &Path,
) -> Result<Vec<DiarizationSegment>, String> {
    let mut empty_segments = None;
    for path in [
        output_dir.join("diarization.json"),
        capture_dir.join("diarization.json"),
    ] {
        if !path.exists() {
            continue;
        }

        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed reading diarization file {}: {e}", path.display()))?;
        let payload = serde_json::from_str::<DiarizationPayload>(&body)
            .map_err(|e| format!("failed parsing diarization file {}: {e}", path.display()))?;
        let segments = match payload {
            DiarizationPayload::Segments(segments) => segments,
            DiarizationPayload::Document { segments } => segments,
        };
        if !segments.is_empty() {
            return Ok(segments);
        }
        empty_segments = Some(segments);
    }

    Ok(empty_segments.unwrap_or_default())
}

fn prepare_diarization_segments(
    capture_dir: &Path,
    output_dir: &Path,
) -> Result<Vec<DiarizationSegment>, String> {
    match read_diarization_segments(capture_dir, output_dir) {
        Ok(segments) if !segments.is_empty() => return Ok(segments),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!("ignoring existing diarization file: {error}");
        }
    }

    let Some(system_audio) = first_existing(capture_dir, &[SYSTEM_AUDIO_FILE])
        .or_else(|| first_existing(output_dir, &[SYSTEM_AUDIO_FILE]))
    else {
        return Ok(Vec::new());
    };

    let output_path = output_dir.join("diarization.json");
    match run_fluid_audio_diarizer(&system_audio, &output_path) {
        Ok(()) => read_diarization_segments(capture_dir, output_dir),
        Err(error) => {
            tracing::warn!("FluidAudioCoreML diarization unavailable: {error}");
            Ok(Vec::new())
        }
    }
}

fn run_fluid_audio_diarizer(system_audio: &Path, output_path: &Path) -> Result<(), String> {
    let binary = resolve_diarizer_binary().ok_or_else(|| {
        let candidates = diarizer_binary_candidates()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("poha-diarizer sidecar not found; checked {candidates}")
    })?;

    let output = std::process::Command::new(&binary)
        .arg("--input")
        .arg(system_audio)
        .arg("--output")
        .arg(output_path)
        .output()
        .map_err(|e| format!("failed spawning {}: {e}", binary.display()))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "poha-diarizer exited without output".to_string()
    };
    Err(format!("{} failed: {detail}", binary.display()))
}

fn resolve_diarizer_binary() -> Option<PathBuf> {
    diarizer_binary_candidates()
        .into_iter()
        .find(|path| path.is_file())
}

fn diarizer_binary_candidates() -> Vec<PathBuf> {
    let target = option_env!("POHA_TARGET_TRIPLE").unwrap_or("aarch64-apple-darwin");
    let env_path = std::env::var("POHA_DIARIZER_BIN").ok().map(PathBuf::from);
    let current_exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(ToOwned::to_owned));
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    diarizer_binary_candidates_for(env_path, current_exe_dir, &manifest_dir, target)
}

fn diarizer_binary_candidates_for(
    env_path: Option<PathBuf>,
    current_exe_dir: Option<PathBuf>,
    manifest_dir: &Path,
    target: &str,
) -> Vec<PathBuf> {
    let suffixed_name = format!("poha-diarizer-{target}");
    let mut candidates = Vec::new();

    if let Some(path) = env_path {
        candidates.push(path);
    }

    if let Some(exe_dir) = current_exe_dir {
        candidates.push(exe_dir.join("poha-diarizer"));
        candidates.push(exe_dir.join(&suffixed_name));
    }

    candidates.push(manifest_dir.join("binaries").join(&suffixed_name));
    candidates.push(
        manifest_dir
            .join("../../..")
            .join("target")
            .join("poha-diarizer-swift")
            .join(target)
            .join("release")
            .join("poha-diarizer"),
    );

    let mut deduped = Vec::new();
    for candidate in candidates {
        if !deduped.contains(&candidate) {
            deduped.push(candidate);
        }
    }
    deduped
}

fn build_segments(
    input: &ProcessInput,
) -> Result<(Vec<TranscriptSegment>, Vec<TranscriptAudioInput>), String> {
    let audio_inputs = labeled_audio_inputs(
        &input.capture_dir,
        input.audio_path_from_event.as_deref(),
        input.settings.speaker_label_mode,
    )
    .map_err(|error| {
        format!(
            "{error} for session {} in {}",
            input.session_id,
            input.capture_dir.display()
        )
    })?;

    let mut segments = Vec::new();
    let audio_input_metadata = audio_inputs
        .iter()
        .map(LabeledAudioInput::metadata)
        .collect::<Vec<_>>();
    let mut chunk_manifest_entries = Vec::new();
    let live_chunks = read_live_chunk_lookup(&input.output_dir).unwrap_or_default();
    for audio_input in audio_inputs {
        transcribe_labeled_with_chunks(
            &audio_input,
            &input.settings.mlx_model,
            &input.settings.mlx_fallback_model,
            &input.output_dir,
            &live_chunks,
            &mut chunk_manifest_entries,
            &mut |mut new_segments| {
                segments.append(&mut new_segments);
                segments.sort_by_key(|segment| (segment.start_ms, segment.source.clone()));
                write_partial_transcript(&input.output_dir, &segments)
            },
        )?;
    }

    segments.sort_by_key(|segment| segment.start_ms);
    if !chunk_manifest_entries.is_empty() {
        write_chunk_manifest(&input.output_dir, chunk_manifest_entries)?;
    }
    Ok((segments, audio_input_metadata))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LabeledAudioInput {
    path: PathBuf,
    speaker: &'static str,
    source: &'static str,
    cache_source: &'static str,
    file_name: &'static str,
    selection: &'static str,
    fallback_reason: Option<String>,
}

impl LabeledAudioInput {
    fn metadata(&self) -> TranscriptAudioInput {
        TranscriptAudioInput {
            speaker: self.speaker.to_string(),
            source: self.source.to_string(),
            file_name: self.file_name.to_string(),
            path: self.path.to_string_lossy().into_owned(),
            selection: self.selection.to_string(),
            fallback_reason: self.fallback_reason.clone(),
        }
    }
}

fn labeled_audio_inputs(
    capture_dir: &Path,
    hinted_audio: Option<&Path>,
    speaker_label_mode: SpeakerLabelMode,
) -> Result<Vec<LabeledAudioInput>, String> {
    if speaker_label_mode == SpeakerLabelMode::MeAndCall {
        let mut inputs = Vec::new();
        let spk = capture_dir.join(SYSTEM_AUDIO_FILE);

        if let Some(mic_input) = select_me_audio_input(capture_dir) {
            inputs.push(mic_input);
        }

        if spk.exists() {
            inputs.push(LabeledAudioInput {
                path: spk,
                speaker: "Call",
                source: "system",
                cache_source: "system",
                file_name: SYSTEM_AUDIO_FILE,
                selection: "systemStem",
                fallback_reason: None,
            });
        }

        if !inputs.is_empty() {
            return Ok(inputs);
        }
    }

    let audio_path = select_primary_audio(capture_dir, hinted_audio)
        .ok_or_else(|| "no capture audio found".to_string())?;
    Ok(vec![LabeledAudioInput {
        path: audio_path,
        speaker: "Speaker",
        source: "mixed",
        cache_source: "mixed",
        file_name: "audio.mp3",
        selection: "mixedAudio",
        fallback_reason: None,
    }])
}

fn select_me_audio_input(capture_dir: &Path) -> Option<LabeledAudioInput> {
    let raw = capture_dir.join(RAW_MIC_FILE);
    let processed = capture_dir.join(PROCESSED_MIC_FILE);
    let system = capture_dir.join(SYSTEM_AUDIO_FILE);
    let raw_exists = raw.exists();
    let processed_exists = processed.exists();

    if processed_exists {
        match validate_processed_mic(
            &processed,
            raw_exists.then_some(raw.as_path()),
            system.exists().then_some(system.as_path()),
        ) {
            Ok(report) if report.healthy => {
                return Some(LabeledAudioInput {
                    path: processed,
                    speaker: "Me",
                    source: "mic",
                    cache_source: "mic_processed",
                    file_name: PROCESSED_MIC_FILE,
                    selection: "processedMic",
                    fallback_reason: None,
                });
            }
            Ok(report) if raw_exists => {
                return Some(raw_mic_input(raw.clone(), Some(report.reason)));
            }
            Err(error) if raw_exists => {
                return Some(raw_mic_input(raw.clone(), Some(error)));
            }
            Ok(report) => {
                return Some(LabeledAudioInput {
                    path: processed,
                    speaker: "Me",
                    source: "mic",
                    cache_source: "mic_processed",
                    file_name: PROCESSED_MIC_FILE,
                    selection: "processedMicOnly",
                    fallback_reason: Some(report.reason),
                });
            }
            Err(error) => {
                return Some(LabeledAudioInput {
                    path: processed,
                    speaker: "Me",
                    source: "mic",
                    cache_source: "mic_processed",
                    file_name: PROCESSED_MIC_FILE,
                    selection: "processedMicOnly",
                    fallback_reason: Some(error),
                });
            }
        }
    }

    raw_exists.then(|| raw_mic_input(raw, None))
}

fn raw_mic_input(path: PathBuf, fallback_reason: Option<String>) -> LabeledAudioInput {
    LabeledAudioInput {
        path,
        speaker: "Me",
        source: "mic",
        cache_source: "mic",
        file_name: RAW_MIC_FILE,
        selection: if fallback_reason.is_some() {
            "rawMicFallback"
        } else {
            "rawMic"
        },
        fallback_reason,
    }
}

#[derive(Debug, Clone)]
struct ProcessedMicValidation {
    healthy: bool,
    reason: String,
}

#[derive(Debug, Clone)]
struct AudioFileStats {
    sample_rate: u32,
    duration_ms: u64,
    rms_dbfs: f64,
    finite: bool,
}

#[derive(Debug, Clone, Default)]
struct ProcessedMicWindowReport {
    raw_only_windows: u64,
    weak_processed_windows: u64,
}

fn validate_processed_mic(
    processed_path: &Path,
    raw_path: Option<&Path>,
    system_path: Option<&Path>,
) -> Result<ProcessedMicValidation, String> {
    let processed = audio_file_stats(processed_path)?;
    if processed.duration_ms == 0 {
        return Ok(processed_mic_unhealthy("processed mic has no audio frames"));
    }
    if !processed.finite {
        return Ok(processed_mic_unhealthy(
            "processed mic contains invalid samples",
        ));
    }

    let raw = raw_path.map(audio_file_stats).transpose()?;
    let system = system_path.map(audio_file_stats).transpose()?;

    if let Some(raw) = raw.as_ref() {
        if !duration_matches(processed.duration_ms, raw.duration_ms) {
            return Ok(processed_mic_unhealthy(&format!(
                "processed mic duration {}ms differs from raw mic {}ms by more than {}ms",
                processed.duration_ms, raw.duration_ms, PROCESSED_MIC_DURATION_TOLERANCE_MS
            )));
        }
        if processed.sample_rate != raw.sample_rate {
            return Ok(processed_mic_unhealthy(&format!(
                "processed mic sample rate {}Hz differs from raw mic {}Hz",
                processed.sample_rate, raw.sample_rate
            )));
        }
    }

    if let Some(system) = system.as_ref() {
        if !duration_matches(processed.duration_ms, system.duration_ms) {
            return Ok(processed_mic_unhealthy(&format!(
                "processed mic duration {}ms differs from system stem {}ms by more than {}ms",
                processed.duration_ms, system.duration_ms, PROCESSED_MIC_DURATION_TOLERANCE_MS
            )));
        }
        if processed.sample_rate != system.sample_rate {
            return Ok(processed_mic_unhealthy(&format!(
                "processed mic sample rate {}Hz differs from system stem {}Hz",
                processed.sample_rate, system.sample_rate
            )));
        }
    }

    if let Some(raw_path) = raw_path {
        let window_report = processed_mic_window_report(processed_path, raw_path, system_path)?;
        if window_report.raw_only_windows >= 2
            && window_report.weak_processed_windows * 2 >= window_report.raw_only_windows
        {
            return Ok(processed_mic_unhealthy(&format!(
                "processed mic is too quiet in {}/{} raw-only speech windows",
                window_report.weak_processed_windows, window_report.raw_only_windows
            )));
        }
    }

    Ok(ProcessedMicValidation {
        healthy: true,
        reason: format!(
            "processed mic passed validation; duration={}ms, rms={:.1} dBFS",
            processed.duration_ms, processed.rms_dbfs
        ),
    })
}

fn processed_mic_unhealthy(reason: &str) -> ProcessedMicValidation {
    ProcessedMicValidation {
        healthy: false,
        reason: reason.to_string(),
    }
}

fn duration_matches(actual_ms: u64, expected_ms: u64) -> bool {
    actual_ms.abs_diff(expected_ms) <= PROCESSED_MIC_DURATION_TOLERANCE_MS
}

fn audio_file_stats(path: &Path) -> Result<AudioFileStats, String> {
    let source = source_from_path(path)
        .map_err(|error| format!("failed decoding {}: {error}", path.display()))?;
    let sample_rate = u32::from(source.sample_rate());
    let channels = usize::from(u16::from(source.channels()).max(1));
    let mut frames = 0_u64;
    let mut sum_squares = 0.0_f64;
    let mut finite = true;

    for sample in mono_frames(source, channels) {
        frames = frames.saturating_add(1);
        if !sample.is_finite() {
            finite = false;
            continue;
        }
        let value = f64::from(sample);
        sum_squares += value * value;
    }

    let duration_ms = if sample_rate == 0 {
        0
    } else {
        frames.saturating_mul(1_000) / u64::from(sample_rate)
    };
    let rms = if frames == 0 {
        0.0
    } else {
        (sum_squares / frames as f64).sqrt()
    };

    Ok(AudioFileStats {
        sample_rate,
        duration_ms,
        rms_dbfs: dbfs_from_rms(rms),
        finite,
    })
}

fn processed_mic_window_report(
    processed_path: &Path,
    raw_path: &Path,
    system_path: Option<&Path>,
) -> Result<ProcessedMicWindowReport, String> {
    let (mut processed, processed_sample_rate) = mono_audio_iter(processed_path)?;
    let (mut raw, raw_sample_rate) = mono_audio_iter(raw_path)?;
    if processed_sample_rate != raw_sample_rate {
        return Ok(ProcessedMicWindowReport::default());
    }

    let mut system = if let Some(path) = system_path {
        let (iter, system_sample_rate) = mono_audio_iter(path)?;
        if system_sample_rate != processed_sample_rate {
            None
        } else {
            Some(iter)
        }
    } else {
        None
    };

    let window_frames = usize::try_from((processed_sample_rate / 2).max(1)).unwrap_or(1);
    let mut report = ProcessedMicWindowReport::default();

    loop {
        let processed_level = window_dbfs(processed.as_mut(), window_frames);
        let raw_level = window_dbfs(raw.as_mut(), window_frames);
        let system_level = system
            .as_mut()
            .and_then(|iter| window_dbfs(iter.as_mut(), window_frames))
            .unwrap_or(f64::NEG_INFINITY);

        if processed_level.is_none() && raw_level.is_none() {
            break;
        }

        let processed_dbfs = processed_level.unwrap_or(f64::NEG_INFINITY);
        let raw_dbfs = raw_level.unwrap_or(f64::NEG_INFINITY);
        let raw_only_speech =
            raw_dbfs >= RAW_ONLY_MIC_ACTIVE_DBFS && system_level < RAW_ONLY_SYSTEM_QUIET_DBFS;
        if raw_only_speech {
            report.raw_only_windows = report.raw_only_windows.saturating_add(1);
            let attenuation = raw_dbfs - processed_dbfs;
            if processed_dbfs < PROCESSED_MIC_MIN_ACTIVE_DBFS
                || attenuation > MAX_RAW_ONLY_PROCESSED_ATTENUATION_DB
            {
                report.weak_processed_windows = report.weak_processed_windows.saturating_add(1);
            }
        }
    }

    Ok(report)
}

fn mono_audio_iter(path: &Path) -> Result<(Box<dyn Iterator<Item = f32>>, u32), String> {
    let source = source_from_path(path)
        .map_err(|error| format!("failed decoding {}: {error}", path.display()))?;
    let sample_rate = u32::from(source.sample_rate());
    let channels = usize::from(u16::from(source.channels()).max(1));
    Ok((Box::new(mono_frames(source, channels)), sample_rate))
}

fn window_dbfs(samples: &mut dyn Iterator<Item = f32>, max_frames: usize) -> Option<f64> {
    let mut frames = 0_usize;
    let mut sum_squares = 0.0_f64;
    for _ in 0..max_frames {
        let sample = samples.next()?;
        frames += 1;
        if sample.is_finite() {
            let value = f64::from(sample);
            sum_squares += value * value;
        }
    }

    if frames == 0 {
        None
    } else {
        Some(dbfs_from_rms((sum_squares / frames as f64).sqrt()))
    }
}

fn dbfs_from_rms(rms: f64) -> f64 {
    if rms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * rms.log10()
    }
}

fn select_primary_audio(capture_dir: &Path, hinted_audio: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = hinted_audio
        && path.exists()
    {
        return Some(path.to_path_buf());
    }
    first_existing(
        capture_dir,
        &["audio.mp3", "audio.wav", "audio.ogg", "audio.m4a"],
    )
}

fn first_existing(base: &Path, candidates: &[&str]) -> Option<PathBuf> {
    for candidate in candidates {
        let path = base.join(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn transcribe_labeled_with_chunks<F>(
    audio_input: &LabeledAudioInput,
    model: &str,
    fallback_model: &str,
    output_dir: &Path,
    live_chunks: &HashMap<ChunkKey, Vec<TranscriptSegment>>,
    chunk_manifest_entries: &mut Vec<ChunkManifestEntry>,
    on_segments: &mut F,
) -> Result<(), String>
where
    F: FnMut(Vec<TranscriptSegment>) -> Result<(), String>,
{
    let chunk_result = chunk_wav_for_transcription(audio_input, output_dir);
    if let Ok(chunks) = chunk_result
        && chunks.len() > 1
    {
        for (index, chunk) in chunks.into_iter().enumerate() {
            let key = ChunkKey {
                source: audio_input.cache_source.to_string(),
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
            };
            let mut segments = if let Some(cached) = live_chunks.get(&key) {
                cached.clone()
            } else {
                let mut segments = transcribe_labeled(
                    &chunk.path,
                    audio_input.speaker,
                    audio_input.source,
                    model,
                    fallback_model,
                )?;
                offset_chunk_segments(&mut segments, chunk.start_ms, chunk.end_ms);
                segments
            };
            chunk_manifest_entries.push(ChunkManifestEntry {
                speaker: audio_input.speaker.to_string(),
                source: audio_input.source.to_string(),
                path: chunk.path.to_string_lossy().into_owned(),
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                status: ChunkStatus::Done,
                error: None,
            });
            tracing::info!(
                source = audio_input.source,
                chunk = index + 1,
                "transcribed audio chunk"
            );
            if !segments.is_empty() {
                on_segments(std::mem::take(&mut segments))?;
            }
        }
        return Ok(());
    }

    let segments = transcribe_labeled(
        &audio_input.path,
        audio_input.speaker,
        audio_input.source,
        model,
        fallback_model,
    )?;
    on_segments(segments)
}

fn chunk_wav_for_transcription(
    audio_input: &LabeledAudioInput,
    output_dir: &Path,
) -> Result<Vec<AudioChunk>, String> {
    if audio_input
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("wav"))
    {
        return Ok(Vec::new());
    }

    let mut reader = hound::WavReader::open(&audio_input.path)
        .map_err(|e| format!("failed opening wav {}: {e}", audio_input.path.display()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let sample_rate = spec.sample_rate;
    if sample_rate == 0 {
        return Ok(Vec::new());
    }

    let chunk_ranges = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            let samples = reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("failed reading wav samples: {e}"))?;
            let frames = normalized_frame_amplitudes(&samples, channels, |sample| {
                sample as f32 / i16::MAX as f32
            });
            let ranges = detect_speech_chunks(&frames, sample_rate);
            write_i16_chunks(
                &audio_input.path,
                output_dir,
                audio_input.cache_source,
                spec,
                &samples,
                &ranges,
            )?
        }
        (hound::SampleFormat::Float, 32) => {
            let samples = reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("failed reading wav samples: {e}"))?;
            let frames = normalized_frame_amplitudes(&samples, channels, |sample| sample);
            let ranges = detect_speech_chunks(&frames, sample_rate);
            write_f32_chunks(
                &audio_input.path,
                output_dir,
                audio_input.cache_source,
                spec,
                &samples,
                &ranges,
            )?
        }
        _ => return Ok(Vec::new()),
    };

    Ok(chunk_ranges)
}

fn normalized_frame_amplitudes<T, F>(samples: &[T], channels: usize, normalize: F) -> Vec<f32>
where
    T: Copy,
    F: Fn(T) -> f32,
{
    if channels == 0 {
        return Vec::new();
    }
    let mut frames = Vec::with_capacity(samples.len() / channels);
    for frame in samples.chunks(channels) {
        let amplitude = frame
            .iter()
            .map(|sample| normalize(*sample).abs())
            .sum::<f32>()
            / frame.len() as f32;
        frames.push(amplitude);
    }
    frames
}

fn detect_speech_chunks(frames: &[f32], sample_rate: u32) -> Vec<(usize, usize)> {
    const WINDOW_MS: usize = 250;
    const SILENCE_MS: usize = 750;
    const MIN_CHUNK_MS: usize = 3_500;
    const MAX_CHUNK_MS: usize = 30_000;
    const PADDING_MS: usize = 200;

    if frames.is_empty() || sample_rate == 0 {
        return Vec::new();
    }

    let frames_per_window = ((sample_rate as usize * WINDOW_MS) / 1000).max(1);
    let silence_windows = (SILENCE_MS / WINDOW_MS).max(1);
    let min_frames = ((sample_rate as usize * MIN_CHUNK_MS) / 1000).max(1);
    let max_frames = ((sample_rate as usize * MAX_CHUNK_MS) / 1000).max(1);
    let padding_frames = (sample_rate as usize * PADDING_MS) / 1000;

    let mut windows = Vec::new();
    for (index, window) in frames.chunks(frames_per_window).enumerate() {
        let start = index * frames_per_window;
        let end = (start + window.len()).min(frames.len());
        let rms =
            (window.iter().map(|value| value * value).sum::<f32>() / window.len() as f32).sqrt();
        windows.push((start, end, rms));
    }

    let max_rms = windows
        .iter()
        .map(|(_, _, rms)| *rms)
        .fold(0.0_f32, f32::max);
    if max_rms < 0.006 {
        return Vec::new();
    }

    let mut sorted_rms = windows.iter().map(|(_, _, rms)| *rms).collect::<Vec<_>>();
    sorted_rms.sort_by(|left, right| left.total_cmp(right));
    let noise_floor = sorted_rms[sorted_rms.len() / 5];
    let threshold = (noise_floor * 4.0).max(max_rms * 0.08).max(0.006);

    let mut chunks = Vec::new();
    let mut active_start: Option<usize> = None;
    let mut last_speech_index = 0usize;

    for (index, (start, end, rms)) in windows.iter().copied().enumerate() {
        let is_speech = rms >= threshold;
        if is_speech {
            if active_start.is_none() {
                active_start = Some(start);
            }
            last_speech_index = index;
        }

        let Some(chunk_start) = active_start else {
            continue;
        };

        let silence_long_enough =
            !is_speech && index.saturating_sub(last_speech_index) >= silence_windows;
        let max_reached = end.saturating_sub(chunk_start) >= max_frames;
        if silence_long_enough || max_reached {
            let speech_end = if max_reached {
                end
            } else {
                windows[last_speech_index].1
            };
            push_chunk_range(
                &mut chunks,
                frames.len(),
                chunk_start,
                speech_end,
                min_frames,
                padding_frames,
            );
            active_start = None;
        }
    }

    if let Some(chunk_start) = active_start {
        push_chunk_range(
            &mut chunks,
            frames.len(),
            chunk_start,
            windows[last_speech_index].1,
            min_frames,
            padding_frames,
        );
    }

    chunks
}

fn push_chunk_range(
    chunks: &mut Vec<(usize, usize)>,
    total_frames: usize,
    start: usize,
    end: usize,
    min_frames: usize,
    padding_frames: usize,
) {
    if end <= start || end - start < min_frames {
        return;
    }
    let padded_start = start.saturating_sub(padding_frames);
    let padded_end = (end + padding_frames).min(total_frames);
    if let Some((_, previous_end)) = chunks.last_mut()
        && padded_start <= *previous_end
    {
        *previous_end = (*previous_end).max(padded_end);
        return;
    }
    chunks.push((padded_start, padded_end));
}

fn write_i16_chunks(
    input_path: &Path,
    output_dir: &Path,
    source: &str,
    spec: hound::WavSpec,
    samples: &[i16],
    ranges: &[(usize, usize)],
) -> Result<Vec<AudioChunk>, String> {
    write_typed_chunks(
        input_path,
        output_dir,
        source,
        spec,
        samples,
        ranges,
        |writer, sample| writer.write_sample(sample),
    )
}

fn write_f32_chunks(
    input_path: &Path,
    output_dir: &Path,
    source: &str,
    spec: hound::WavSpec,
    samples: &[f32],
    ranges: &[(usize, usize)],
) -> Result<Vec<AudioChunk>, String> {
    write_typed_chunks(
        input_path,
        output_dir,
        source,
        spec,
        samples,
        ranges,
        |writer, sample| writer.write_sample(sample),
    )
}

fn write_typed_chunks<T, F>(
    input_path: &Path,
    output_dir: &Path,
    source: &str,
    spec: hound::WavSpec,
    samples: &[T],
    ranges: &[(usize, usize)],
    mut write_sample: F,
) -> Result<Vec<AudioChunk>, String>
where
    T: Copy,
    F: FnMut(
        &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        T,
    ) -> Result<(), hound::Error>,
{
    if ranges.len() <= 1 {
        return Ok(Vec::new());
    }
    let chunks_dir = output_dir.join("transcript_chunks").join(source);
    std::fs::create_dir_all(&chunks_dir)
        .map_err(|e| format!("failed creating chunks dir {}: {e}", chunks_dir.display()))?;

    let channels = usize::from(spec.channels.max(1));
    let mut chunks = Vec::new();
    for (index, (start_frame, end_frame)) in ranges.iter().copied().enumerate() {
        let path = chunks_dir.join(format!("{source}-{index:04}.wav"));
        chunks.push(write_typed_chunk(
            &path,
            spec,
            channels,
            samples,
            (start_frame, end_frame),
            &mut write_sample,
        )?);
    }

    tracing::info!(
        source,
        chunks = chunks.len(),
        input = %input_path.display(),
        "prepared transcript audio chunks"
    );
    Ok(chunks)
}

fn write_typed_chunk<T, F>(
    path: &Path,
    spec: hound::WavSpec,
    channels: usize,
    samples: &[T],
    range: (usize, usize),
    write_sample: &mut F,
) -> Result<AudioChunk, String>
where
    T: Copy,
    F: FnMut(
        &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        T,
    ) -> Result<(), hound::Error>,
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed creating chunks dir {}: {e}", parent.display()))?;
    }

    let (start_frame, end_frame) = range;
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("failed creating chunk {}: {e}", path.display()))?;
    let start_sample = start_frame.saturating_mul(channels);
    let end_sample = end_frame.saturating_mul(channels).min(samples.len());
    for sample in &samples[start_sample..end_sample] {
        write_sample(&mut writer, *sample)
            .map_err(|e| format!("failed writing chunk {}: {e}", path.display()))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("failed finalizing chunk {}: {e}", path.display()))?;

    Ok(AudioChunk {
        path: path.to_path_buf(),
        start_ms: frames_to_ms(start_frame, spec.sample_rate),
        end_ms: frames_to_ms(end_frame, spec.sample_rate),
    })
}

fn frames_to_ms(frames: usize, sample_rate: u32) -> i64 {
    ((frames as f64 / sample_rate.max(1) as f64) * 1000.0).round() as i64
}

fn offset_chunk_segments(
    segments: &mut [TranscriptSegment],
    chunk_start_ms: i64,
    chunk_end_ms: i64,
) {
    for segment in segments {
        if segment.start_ms == 0 && segment.end_ms == 0 {
            segment.start_ms = chunk_start_ms;
            segment.end_ms = chunk_end_ms;
        } else {
            segment.start_ms += chunk_start_ms;
            segment.end_ms += chunk_start_ms;
        }
    }
}

fn write_partial_transcript(
    output_dir: &Path,
    segments: &[TranscriptSegment],
) -> Result<(), String> {
    let partial_path = output_dir.join("transcript.partial.md");
    std::fs::write(&partial_path, render_markdown(segments)).map_err(|e| {
        format!(
            "failed writing partial transcript {}: {e}",
            partial_path.display()
        )
    })
}

fn write_chunk_manifest(output_dir: &Path, chunks: Vec<ChunkManifestEntry>) -> Result<(), String> {
    let manifest = ChunkManifest {
        strategy: "energy-vad-wav-chunks".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        chunks,
    };
    let path = output_dir.join("transcript.chunks.json");
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("failed serializing chunk manifest: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("failed writing chunk manifest {}: {e}", path.display()))
}

fn run_live_transcription(
    input: ProcessInput,
    stop_requested: Arc<AtomicBool>,
) -> Result<(), String> {
    std::fs::create_dir_all(&input.output_dir).map_err(|e| {
        format!(
            "failed creating live transcription output dir {}: {e}",
            input.output_dir.display()
        )
    })?;

    let mut snapshot =
        read_live_chunk_snapshot(&input.output_dir).unwrap_or_else(|_| new_live_chunk_snapshot());

    loop {
        let stopping = stop_requested.load(Ordering::SeqCst);
        if let Err(error) = process_live_transcription_pass(&input, &mut snapshot, stopping) {
            tracing::warn!("live transcription pass failed: {error}");
        }

        if stopping {
            break;
        }

        std::thread::sleep(Duration::from_millis(900));
    }

    Ok(())
}

fn process_live_transcription_pass(
    input: &ProcessInput,
    snapshot: &mut LiveChunkSnapshot,
    include_tail: bool,
) -> Result<(), String> {
    let audio_inputs = match labeled_audio_inputs(
        &input.capture_dir,
        input.audio_path_from_event.as_deref(),
        input.settings.speaker_label_mode,
    ) {
        Ok(inputs) => inputs,
        Err(error) if error == "no capture audio found" => return Ok(()),
        Err(error) => return Err(error),
    };

    let mut changed = false;
    for audio_input in audio_inputs {
        changed |= process_live_audio_input(
            &audio_input,
            &input.settings.mlx_model,
            &input.settings.mlx_fallback_model,
            &input.output_dir,
            snapshot,
            include_tail,
        )?;
    }

    if changed {
        write_live_outputs(&input.output_dir, snapshot)?;
    }

    Ok(())
}

fn process_live_audio_input(
    audio_input: &LabeledAudioInput,
    model: &str,
    fallback_model: &str,
    output_dir: &Path,
    snapshot: &mut LiveChunkSnapshot,
    include_tail: bool,
) -> Result<bool, String> {
    let Some(partial) = read_partial_wav(&audio_input.path)? else {
        return Ok(false);
    };

    let ranges = detect_speech_chunks(&partial.frames, partial.spec.sample_rate);
    if ranges.is_empty() {
        return Ok(false);
    }

    let channels = usize::from(partial.spec.channels.max(1));
    let close_lag_frames = (partial.spec.sample_rate as usize * 1_000) / 1000;
    let mut changed = false;

    for (index, (start_frame, end_frame)) in ranges.into_iter().enumerate() {
        if !include_tail && partial.frames.len().saturating_sub(end_frame) < close_lag_frames.max(1)
        {
            continue;
        }

        let start_ms = frames_to_ms(start_frame, partial.spec.sample_rate);
        let end_ms = frames_to_ms(end_frame, partial.spec.sample_rate);
        if snapshot.chunks.iter().any(|entry| {
            entry.source == audio_input.cache_source
                && entry.start_ms == start_ms
                && entry.end_ms == end_ms
        }) {
            continue;
        }

        let chunk_path = output_dir
            .join("transcript_chunks")
            .join(audio_input.cache_source)
            .join(format!("{}-{index:04}.wav", audio_input.cache_source));
        let chunk = write_partial_chunk(&partial, &chunk_path, channels, (start_frame, end_frame))?;

        let entry_index = snapshot.chunks.len();
        snapshot.chunks.push(LiveChunkEntry {
            speaker: audio_input.speaker.to_string(),
            source: audio_input.cache_source.to_string(),
            path: chunk.path.to_string_lossy().into_owned(),
            start_ms: chunk.start_ms,
            end_ms: chunk.end_ms,
            status: ChunkStatus::Queued,
            error: None,
            segments: Vec::new(),
        });
        write_live_outputs(output_dir, snapshot)?;

        snapshot.chunks[entry_index].status = ChunkStatus::Transcribing;
        snapshot.chunks[entry_index].error = None;
        write_live_outputs(output_dir, snapshot)?;

        match transcribe_labeled(
            &chunk.path,
            audio_input.speaker,
            audio_input.source,
            model,
            fallback_model,
        ) {
            Ok(mut segments) => {
                offset_chunk_segments(&mut segments, chunk.start_ms, chunk.end_ms);
                snapshot.chunks[entry_index].status = ChunkStatus::Done;
                snapshot.chunks[entry_index].segments = segments;
                snapshot.chunks[entry_index].error = None;
            }
            Err(error) => {
                snapshot.chunks[entry_index].status = ChunkStatus::Failed;
                snapshot.chunks[entry_index].segments.clear();
                snapshot.chunks[entry_index].error = Some(error);
            }
        }
        changed = true;
        write_live_outputs(output_dir, snapshot)?;
    }

    Ok(changed)
}

fn write_partial_chunk(
    partial: &PartialWav,
    path: &Path,
    channels: usize,
    range: (usize, usize),
) -> Result<AudioChunk, String> {
    match &partial.samples {
        PartialSamples::I16(samples) => {
            let mut write_sample =
                |writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>, sample: i16| {
                    writer.write_sample(sample)
                };
            write_typed_chunk(
                path,
                partial.spec,
                channels,
                samples,
                range,
                &mut write_sample,
            )
        }
        PartialSamples::F32(samples) => {
            let mut write_sample =
                |writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>, sample: f32| {
                    writer.write_sample(sample)
                };
            write_typed_chunk(
                path,
                partial.spec,
                channels,
                samples,
                range,
                &mut write_sample,
            )
        }
    }
}

fn write_live_outputs(output_dir: &Path, snapshot: &mut LiveChunkSnapshot) -> Result<(), String> {
    snapshot.updated_at = Utc::now().to_rfc3339();
    write_live_chunk_snapshot(output_dir, snapshot)?;

    let mut manifest_entries = snapshot
        .chunks
        .iter()
        .map(|entry| ChunkManifestEntry {
            speaker: entry.speaker.clone(),
            source: entry.source.clone(),
            path: entry.path.clone(),
            start_ms: entry.start_ms,
            end_ms: entry.end_ms,
            status: entry.status,
            error: entry.error.clone(),
        })
        .collect::<Vec<_>>();
    manifest_entries.sort_by_key(|entry| {
        (
            entry.start_ms,
            source_sort_key(&entry.source),
            entry.end_ms,
            entry.path.clone(),
        )
    });
    write_chunk_manifest(output_dir, manifest_entries)?;

    let mut segments = snapshot
        .chunks
        .iter()
        .filter(|entry| entry.status == ChunkStatus::Done)
        .flat_map(|entry| entry.segments.clone())
        .collect::<Vec<_>>();
    segments.sort_by_key(|segment| (segment.start_ms, source_sort_key(&segment.source)));
    if !segments.is_empty() {
        write_partial_transcript(output_dir, &segments)?;
    }

    Ok(())
}

fn source_sort_key(source: &str) -> u8 {
    match source {
        "mic" => 0,
        "mic_processed" => 0,
        "system" => 1,
        _ => 2,
    }
}

fn read_live_chunk_lookup(
    output_dir: &Path,
) -> Result<HashMap<ChunkKey, Vec<TranscriptSegment>>, String> {
    let snapshot = read_live_chunk_snapshot(output_dir)?;
    Ok(snapshot
        .chunks
        .into_iter()
        .filter(|entry| entry.status == ChunkStatus::Done && !entry.segments.is_empty())
        .map(|entry| {
            (
                ChunkKey {
                    source: entry.source,
                    start_ms: entry.start_ms,
                    end_ms: entry.end_ms,
                },
                entry.segments,
            )
        })
        .collect())
}

fn new_live_chunk_snapshot() -> LiveChunkSnapshot {
    let now = Utc::now().to_rfc3339();
    LiveChunkSnapshot {
        strategy: "energy-vad-wav-chunks-live".to_string(),
        generated_at: now.clone(),
        updated_at: now,
        chunks: Vec::new(),
    }
}

fn live_chunk_snapshot_path(output_dir: &Path) -> PathBuf {
    output_dir.join("transcript.live.json")
}

fn read_live_chunk_snapshot(output_dir: &Path) -> Result<LiveChunkSnapshot, String> {
    let path = live_chunk_snapshot_path(output_dir);
    let body = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "failed reading live transcript snapshot {}: {e}",
            path.display()
        )
    })?;
    serde_json::from_str(&body).map_err(|e| {
        format!(
            "failed parsing live transcript snapshot {}: {e}",
            path.display()
        )
    })
}

fn write_live_chunk_snapshot(
    output_dir: &Path,
    snapshot: &LiveChunkSnapshot,
) -> Result<(), String> {
    let path = live_chunk_snapshot_path(output_dir);
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| format!("failed serializing live transcript snapshot: {e}"))?;
    std::fs::write(&path, json).map_err(|e| {
        format!(
            "failed writing live transcript snapshot {}: {e}",
            path.display()
        )
    })
}

fn read_partial_wav(path: &Path) -> Result<Option<PartialWav>, String> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("wav"))
    {
        return Ok(None);
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed reading partial wav {}: {error}",
                path.display()
            ));
        }
    };
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(None);
    }

    let Some((fmt, data_start)) = parse_wav_header(&bytes) else {
        return Ok(None);
    };
    let sample_width = usize::from(fmt.bits_per_sample / 8);
    if sample_width == 0 || fmt.channels == 0 || fmt.sample_rate == 0 {
        return Ok(None);
    }

    let channels = usize::from(fmt.channels);
    let available_bytes = bytes.len().saturating_sub(data_start);
    let usable_sample_count = (available_bytes / sample_width / channels) * channels;
    if usable_sample_count == 0 {
        return Ok(None);
    }
    let usable_bytes = usable_sample_count * sample_width;
    let sample_bytes = &bytes[data_start..data_start + usable_bytes];

    let spec = hound::WavSpec {
        channels: fmt.channels,
        sample_rate: fmt.sample_rate,
        bits_per_sample: fmt.bits_per_sample,
        sample_format: fmt.sample_format,
    };

    match (fmt.sample_format, fmt.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            let samples = sample_bytes
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            let frames = normalized_frame_amplitudes(&samples, channels, |sample| {
                sample as f32 / i16::MAX as f32
            });
            Ok(Some(PartialWav {
                spec,
                frames,
                samples: PartialSamples::I16(samples),
            }))
        }
        (hound::SampleFormat::Float, 32) => {
            let samples = sample_bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect::<Vec<_>>();
            let frames = normalized_frame_amplitudes(&samples, channels, |sample| sample);
            Ok(Some(PartialWav {
                spec,
                frames,
                samples: PartialSamples::F32(samples),
            }))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedWavFormat {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    sample_format: hound::SampleFormat,
}

fn parse_wav_header(bytes: &[u8]) -> Option<(ParsedWavFormat, usize)> {
    let mut cursor = 12usize;
    let mut format = None;
    let mut data_start = None;

    while cursor.checked_add(8)? <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]) as usize;
        let content_start = cursor + 8;
        if chunk_id == b"fmt " && content_start + 16 <= bytes.len() {
            let audio_format = u16::from_le_bytes([bytes[content_start], bytes[content_start + 1]]);
            let channels = u16::from_le_bytes([bytes[content_start + 2], bytes[content_start + 3]]);
            let sample_rate = u32::from_le_bytes([
                bytes[content_start + 4],
                bytes[content_start + 5],
                bytes[content_start + 6],
                bytes[content_start + 7],
            ]);
            let bits_per_sample =
                u16::from_le_bytes([bytes[content_start + 14], bytes[content_start + 15]]);
            let sample_format = match audio_format {
                1 => hound::SampleFormat::Int,
                3 => hound::SampleFormat::Float,
                _ => return None,
            };
            format = Some(ParsedWavFormat {
                channels,
                sample_rate,
                bits_per_sample,
                sample_format,
            });
        } else if chunk_id == b"data" {
            data_start = Some(content_start);
            break;
        }

        let padded_len = chunk_len + (chunk_len % 2);
        cursor = content_start.checked_add(padded_len)?;
    }

    Some((format?, data_start?))
}

fn transcribe_labeled(
    path: &Path,
    speaker: &str,
    source: &str,
    model: &str,
    fallback_model: &str,
) -> Result<Vec<TranscriptSegment>, String> {
    let primary_attempt = resolve_model_reference(model).and_then(|resolved| {
        run_mlx_whisper(path, &resolved)
            .map_err(|error| format!("{error} (model={model}, resolved={resolved})"))
    });
    match primary_attempt {
        Ok(output) => Ok(convert_output_to_segments(output, speaker, source)),
        Err(primary_error) => {
            if fallback_model.trim().is_empty() || fallback_model == model {
                return Err(primary_error);
            }
            let resolved_fallback = resolve_model_reference(fallback_model)?;
            let output = run_mlx_whisper(path, &resolved_fallback).map_err(|fallback_error| {
                format!(
                    "{primary_error}; fallback failed: {fallback_error} (fallback={fallback_model}, resolved={resolved_fallback})"
                )
            })?;
            Ok(convert_output_to_segments(output, speaker, source))
        }
    }
}

fn run_mlx_whisper(path: &Path, model: &str) -> Result<MlxOutput, String> {
    let script = format!(
        r#"
import json
import math
import mlx_whisper

result = mlx_whisper.transcribe({audio_path:?}, path_or_hf_repo={model:?})

def finite_number(value):
    if value is None:
        return None
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(number):
        return None
    return number

def clean_text(value):
    if value is None:
        return ""
    return str(value).strip()

segments = []
for segment in result.get("segments") or []:
    if not isinstance(segment, dict):
        continue
    text = clean_text(segment.get("text"))
    if not text:
        continue
    segments.append({{
        "start": finite_number(segment.get("start")),
        "end": finite_number(segment.get("end")),
        "text": text,
    }})

payload = {{
    "text": clean_text(result.get("text")),
    "segments": segments,
}}
print(json.dumps(payload, allow_nan=False))
"#,
        audio_path = path.to_string_lossy(),
        model = model,
    );

    let uvx = resolve_uvx_executable().ok_or_else(|| {
        "uvx not found. Checked app PATH plus ~/.local/bin, ~/.cargo/bin, /opt/homebrew/bin, and /usr/local/bin".to_string()
    })?;
    let args = mlx_command_args(&script);
    let output = std::process::Command::new(&uvx)
        .env("PATH", command_path_env())
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn {}: {e}", uvx.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            "mlx-whisper transcription failed".to_string()
        } else {
            stderr
        };
        return Err(message);
    }

    parse_mlx_output_stdout(&output.stdout)
}

fn parse_mlx_output_stdout(stdout: &[u8]) -> Result<MlxOutput, String> {
    let stdout = String::from_utf8_lossy(stdout);
    let json_line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| "mlx-whisper produced no JSON output".to_string())?;

    serde_json::from_str::<MlxOutput>(json_line)
        .map_err(|e| format!("failed to parse mlx-whisper output: {e}"))
}

fn mlx_command_args(script: &str) -> Vec<String> {
    vec![
        "--with".to_string(),
        "mlx-whisper".to_string(),
        "python".to_string(),
        "-c".to_string(),
        script.to_string(),
    ]
}

fn resolve_uvx_executable() -> Option<PathBuf> {
    resolve_uvx_executable_from_paths(&command_search_paths())
}

fn resolve_uvx_executable_from_paths(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .map(|path| path.join("uvx"))
        .find(|candidate| candidate.is_file())
}

fn command_path_env() -> std::ffi::OsString {
    std::env::join_paths(command_search_paths()).unwrap_or_else(|_| {
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into())
    })
}

fn command_search_paths() -> Vec<PathBuf> {
    let mut paths = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin"));
        paths.push(home.join(".cargo/bin"));
    }

    paths.push(PathBuf::from("/opt/homebrew/bin"));
    paths.push(PathBuf::from("/usr/local/bin"));
    paths.push(PathBuf::from("/usr/bin"));
    paths.push(PathBuf::from("/bin"));

    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.contains(&path) {
            deduped.push(path);
        }
    }
    deduped
}

fn resolve_model_reference(model: &str) -> Result<String, String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err("model is empty".to_string());
    }

    let path_candidate = PathBuf::from(trimmed);
    if path_candidate.exists() {
        validate_local_mlx_model_dir(trimmed, &path_candidate)?;
        return Ok(path_candidate.to_string_lossy().into_owned());
    }

    if let Some(snapshot) = resolve_hf_snapshot_dir(trimmed) {
        validate_local_mlx_model_dir(trimmed, &snapshot)?;
        return Ok(snapshot.to_string_lossy().into_owned());
    }

    Ok(trimmed.to_string())
}

fn validate_local_mlx_model_dir(model_label: &str, model_dir: &Path) -> Result<(), String> {
    if !model_dir.is_dir() {
        return Err(format!(
            "model path for {model_label} is not a directory: {}",
            model_dir.display()
        ));
    }

    let weights_npz = model_dir.join("weights.npz");
    let weights_safetensors = model_dir.join("weights.safetensors");
    if weights_npz.exists() || weights_safetensors.exists() {
        return Ok(());
    }

    let safetensors = model_dir.join("model.safetensors");
    if safetensors.exists() {
        return Err(format!(
            "model {model_label} at {} has model.safetensors but no weights.npz/weights.safetensors; use a mlx-whisper-compatible model (for example mlx-community/whisper-large-v3-turbo or mlx-community/whisper-turbo)",
            model_dir.display()
        ));
    }

    Err(format!(
        "model {model_label} at {} missing weights.npz/weights.safetensors",
        model_dir.display()
    ))
}

fn resolve_hf_snapshot_dir(model: &str) -> Option<PathBuf> {
    let cache_root = huggingface_hub_cache_dir()?;
    let normalized = model.replace('/', "--");
    let repo_root = cache_root.join(format!("models--{normalized}"));
    let revision = std::fs::read_to_string(repo_root.join("refs").join("main"))
        .ok()?
        .trim()
        .to_string();
    if revision.is_empty() {
        return None;
    }
    let snapshot = repo_root.join("snapshots").join(revision);
    if snapshot.exists() {
        Some(snapshot)
    } else {
        None
    }
}

fn huggingface_hub_cache_dir() -> Option<PathBuf> {
    if let Ok(explicit_cache) = std::env::var("HUGGINGFACE_HUB_CACHE") {
        let path = PathBuf::from(explicit_cache);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(hf_home) = std::env::var("HF_HOME") {
        let path = PathBuf::from(hf_home).join("hub");
        if path.exists() {
            return Some(path);
        }
    }

    dirs::home_dir()
        .map(|home| home.join(".cache").join("huggingface").join("hub"))
        .filter(|path| path.exists())
}

fn convert_output_to_segments(
    output: MlxOutput,
    speaker: &str,
    source: &str,
) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();

    if let Some(raw_segments) = output.segments {
        for segment in raw_segments {
            let text = segment.text.unwrap_or_default().trim().to_string();
            if text.is_empty() {
                continue;
            }
            segments.push(TranscriptSegment {
                speaker: speaker.to_string(),
                speaker_id: None,
                source: source.to_string(),
                start_ms: seconds_to_ms(segment.start.unwrap_or_default()),
                end_ms: seconds_to_ms(segment.end.unwrap_or_default()),
                text,
            });
        }
    }

    if segments.is_empty() {
        let text = output.text.unwrap_or_default().trim().to_string();
        if !text.is_empty() {
            segments.push(TranscriptSegment {
                speaker: speaker.to_string(),
                speaker_id: None,
                source: source.to_string(),
                start_ms: 0,
                end_ms: 0,
                text,
            });
        }
    }

    segments
}

fn seconds_to_ms(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

fn apply_system_diarization(
    segments: &mut [TranscriptSegment],
    diarization_segments: &[DiarizationSegment],
) {
    if diarization_segments.is_empty() {
        return;
    }

    let label_map = speaker_label_map(diarization_segments);
    for segment in segments {
        if segment.source != "system" {
            continue;
        }

        if let Some(diarization) = best_diarization_segment(segment, diarization_segments) {
            segment.speaker_id = Some(diarization.speaker_id.to_string());
            segment.speaker = label_map
                .get(&diarization.speaker_id)
                .cloned()
                .unwrap_or_else(|| "Speaker".to_string());
            if should_snap_system_timestamp(segment, diarization) {
                segment.start_ms = diarization.start_ms;
                segment.end_ms = diarization.end_ms;
            }
        }
    }
}

fn speaker_label_map(
    segments: &[DiarizationSegment],
) -> std::collections::BTreeMap<String, String> {
    let mut labels = std::collections::BTreeMap::new();
    let mut ordered = segments
        .iter()
        .filter(|segment| segment.end_ms > segment.start_ms)
        .collect::<Vec<_>>();
    ordered.sort_by_key(|segment| (segment.start_ms, segment.end_ms, segment.speaker_id.clone()));

    for segment in ordered {
        if labels.contains_key(&segment.speaker_id) {
            continue;
        }

        let label = format!("Speaker {}", labels.len() + 1);
        labels.insert(segment.speaker_id.clone(), label);
    }

    labels
}

fn best_diarization_segment<'a>(
    segment: &TranscriptSegment,
    diarization_segments: &'a [DiarizationSegment],
) -> Option<&'a DiarizationSegment> {
    const NEAREST_GAP_MS: i64 = 1_500;

    let mut best_overlap: Option<(&DiarizationSegment, i64, i64)> = None;
    for diarization in diarization_segments {
        if diarization.end_ms <= diarization.start_ms {
            continue;
        }

        let overlap = time_overlap_ms(
            segment.start_ms,
            segment.end_ms,
            diarization.start_ms,
            diarization.end_ms,
        );
        if overlap <= 0 {
            continue;
        }

        let duration = diarization.end_ms - diarization.start_ms;
        if best_overlap.is_none_or(|(_, current_overlap, current_duration)| {
            overlap > current_overlap || (overlap == current_overlap && duration > current_duration)
        }) {
            best_overlap = Some((diarization, overlap, duration));
        }
    }

    if let Some((diarization, _, _)) = best_overlap {
        return Some(diarization);
    }

    diarization_segments
        .iter()
        .filter(|diarization| diarization.end_ms > diarization.start_ms)
        .map(|diarization| {
            (
                diarization,
                time_gap_ms(
                    segment.start_ms,
                    segment.end_ms,
                    diarization.start_ms,
                    diarization.end_ms,
                ),
            )
        })
        .filter(|(_, gap)| *gap <= NEAREST_GAP_MS)
        .min_by_key(|(_, gap)| *gap)
        .map(|(diarization, _)| diarization)
}

fn should_snap_system_timestamp(
    segment: &TranscriptSegment,
    diarization: &DiarizationSegment,
) -> bool {
    const START_OF_FILE_MS: i64 = 1_000;
    const LARGE_LEADING_SILENCE_MS: i64 = 3_000;

    if segment.source != "system" || diarization.end_ms <= diarization.start_ms {
        return false;
    }
    if segment.end_ms <= segment.start_ms {
        return true;
    }

    let leading_silence = diarization.start_ms.saturating_sub(segment.start_ms);
    segment.start_ms <= START_OF_FILE_MS && leading_silence >= LARGE_LEADING_SILENCE_MS
}

fn time_overlap_ms(left_start: i64, left_end: i64, right_start: i64, right_end: i64) -> i64 {
    left_end.min(right_end) - left_start.max(right_start)
}

fn time_gap_ms(left_start: i64, left_end: i64, right_start: i64, right_end: i64) -> i64 {
    if left_end < right_start {
        right_start - left_end
    } else if right_end < left_start {
        left_start - right_end
    } else {
        0
    }
}

fn suppress_system_bleed_duplicates(segments: &mut Vec<TranscriptSegment>) {
    let system_segments = segments
        .iter()
        .filter(|segment| segment.source == "system")
        .cloned()
        .collect::<Vec<_>>();
    if system_segments.is_empty() {
        return;
    }

    segments.retain(|segment| !is_system_bleed_duplicate(segment, &system_segments));
    segments.sort_by_key(|segment| (segment.start_ms, source_sort_key(&segment.source)));
}

fn is_system_bleed_duplicate(
    segment: &TranscriptSegment,
    system_segments: &[TranscriptSegment],
) -> bool {
    if segment.source != "mic" || segment.end_ms <= segment.start_ms {
        return false;
    }

    system_segments.iter().any(|system| {
        let overlap_ms = time_overlap_ms(
            segment.start_ms,
            segment.end_ms,
            system.start_ms,
            system.end_ms,
        );
        let duration_ms = segment.end_ms.saturating_sub(segment.start_ms).max(1);
        overlap_ms > 0
            && overlap_ms.saturating_mul(100) >= duration_ms.saturating_mul(60)
            && likely_same_transcript_text(&segment.text, &system.text)
    })
}

fn likely_same_transcript_text(left: &str, right: &str) -> bool {
    let left_tokens = significant_text_tokens(left);
    let right_tokens = significant_text_tokens(right);
    if left_tokens.len().min(right_tokens.len()) < 3 {
        return false;
    }

    let common = common_token_count(&left_tokens, &right_tokens);
    common >= 3
        && common as f32 / left_tokens.len() as f32 >= 0.55
        && common as f32 / right_tokens.len() as f32 >= 0.5
}

fn significant_text_tokens(text: &str) -> Vec<String> {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|token| token.len() > 1)
        .filter(|token| !matches!(*token, "ah" | "eh" | "er" | "hm" | "oh" | "uh" | "um"))
        .map(ToString::to_string)
        .collect()
}

fn common_token_count(left: &[String], right: &[String]) -> usize {
    let mut right_counts = std::collections::BTreeMap::<&str, usize>::new();
    for token in right {
        *right_counts.entry(token.as_str()).or_default() += 1;
    }

    let mut common = 0usize;
    for token in left {
        let Some(count) = right_counts.get_mut(token.as_str()) else {
            continue;
        };
        if *count > 0 {
            *count -= 1;
            common += 1;
        }
    }
    common
}

fn display_segments(segments: &[TranscriptSegment]) -> Vec<TranscriptSegment> {
    const MERGE_GAP_MS: i64 = 1_500;

    let mut display = Vec::<TranscriptSegment>::new();
    let mut sorted = segments.to_vec();
    sorted.sort_by_key(|segment| (segment.start_ms, segment.end_ms, segment.source.clone()));

    for segment in sorted {
        if segment.text.trim().is_empty() {
            continue;
        }

        if let Some(previous) = display.last_mut()
            && previous.speaker == segment.speaker
            && previous.speaker_id == segment.speaker_id
            && previous.source == segment.source
            && segment.start_ms.saturating_sub(previous.end_ms) <= MERGE_GAP_MS
        {
            if !previous
                .text
                .chars()
                .last()
                .is_some_and(char::is_whitespace)
            {
                previous.text.push(' ');
            }
            previous.text.push_str(segment.text.trim());
            previous.end_ms = previous.end_ms.max(segment.end_ms);
            continue;
        }

        display.push(segment);
    }

    display
}

pub fn render_markdown(segments: &[TranscriptSegment]) -> String {
    let display_segments = display_segments(segments);
    let mut lines = Vec::with_capacity(display_segments.len() + 2);
    lines.push("# Transcript".to_string());
    lines.push(String::new());

    for segment in &display_segments {
        lines.push(format!(
            "[{}] {}: {}",
            format_mm_ss(segment.start_ms),
            segment.speaker,
            segment.text
        ));
    }

    if display_segments.is_empty() {
        lines.push("No transcript produced.".to_string());
    }

    lines.push(String::new());
    lines.join("\n")
}

fn format_mm_ss(ms: i64) -> String {
    let total_seconds = (ms.max(0) / 1000) as u64;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn transcript_segment(
        speaker: &str,
        source: &str,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> TranscriptSegment {
        TranscriptSegment {
            speaker: speaker.to_string(),
            speaker_id: None,
            source: source.to_string(),
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    #[test]
    fn mlx_args_use_batch_python_path_not_websocket() {
        let args = mlx_command_args("print('ok')");
        assert_eq!(args[0], "--with");
        assert_eq!(args[1], "mlx-whisper");
        assert_eq!(args[2], "python");
        assert_eq!(args[3], "-c");
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("ws://") || arg.starts_with("wss://"))
        );
    }

    #[test]
    fn uvx_resolution_checks_expanded_paths() {
        let dir = tempdir().expect("temp dir");
        let uvx = dir.path().join("uvx");
        std::fs::write(&uvx, "#!/bin/sh\n").expect("write uvx");

        assert_eq!(
            resolve_uvx_executable_from_paths(&[dir.path().to_path_buf()]),
            Some(uvx)
        );
    }

    #[test]
    fn mlx_output_parser_uses_last_json_line() {
        let output = parse_mlx_output_stdout(
            br#"downloading model...
{"text":"hello","segments":[{"start":1.0,"end":2.0,"text":"hello"}]}
"#,
        )
        .expect("parse output");

        assert_eq!(output.text.as_deref(), Some("hello"));
        assert_eq!(output.segments.expect("segments").len(), 1);
    }

    #[test]
    fn mlx_output_parser_reports_missing_json() {
        let error = parse_mlx_output_stdout(b"downloading model...\n").expect_err("parse error");
        assert_eq!(error, "mlx-whisper produced no JSON output");
    }

    #[test]
    fn copy_audio_artifacts_copies_required_files_when_present() {
        let src = tempdir().expect("temp src");
        let dst = tempdir().expect("temp dst");

        for (name, body) in [
            ("audio.mp3", b"mix".as_slice()),
            ("audio_mic.wav", b"mic".as_slice()),
            ("audio_mic_processed.wav", b"processed".as_slice()),
            ("audio_spk.wav", b"spk".as_slice()),
        ] {
            std::fs::write(src.path().join(name), body).expect("write source artifact");
        }

        copy_audio_artifacts(src.path(), dst.path()).expect("copy artifacts");

        assert!(dst.path().join("audio.mp3").exists());
        assert!(dst.path().join("audio_mic.wav").exists());
        assert!(dst.path().join("audio_mic_processed.wav").exists());
        assert!(dst.path().join("audio_spk.wav").exists());
    }

    #[test]
    fn render_markdown_includes_timestamps_and_speaker_labels() {
        let markdown = render_markdown(&[transcript_segment("Me", "mic", 12_000, 14_000, "hello")]);
        assert!(markdown.contains("# Transcript"));
        assert!(markdown.contains("[00:12] Me: hello"));
    }

    #[test]
    fn render_markdown_consolidates_adjacent_same_speaker_segments() {
        let markdown = render_markdown(&[
            transcript_segment("Speaker 1", "system", 1_000, 2_000, "hello"),
            transcript_segment("Speaker 1", "system", 2_500, 3_000, "there"),
            transcript_segment("Speaker 2", "system", 3_100, 4_000, "reply"),
        ]);

        assert!(markdown.contains("[00:01] Speaker 1: hello there"));
        assert!(markdown.contains("[00:03] Speaker 2: reply"));
    }

    #[test]
    fn suppresses_mic_segment_that_duplicates_overlapping_system_audio() {
        let mut segments = vec![
            transcript_segment(
                "Me",
                "mic",
                0,
                10_560,
                "oh my microphone check I am doing check on the boa microphone",
            ),
            transcript_segment("Speaker 1", "system", 0, 15_680, "POHA system audio check."),
            transcript_segment("Me", "mic", 10_560, 16_080, "oh his system audio check"),
        ];

        suppress_system_bleed_duplicates(&mut segments);

        assert_eq!(segments.len(), 2);
        assert!(
            segments
                .iter()
                .all(|segment| segment.text != "oh his system audio check")
        );
        assert!(
            segments
                .iter()
                .any(|segment| segment.text.contains("microphone check"))
        );
    }

    #[test]
    fn keeps_overlapping_mic_speech_that_is_not_a_system_duplicate() {
        let mut segments = vec![
            transcript_segment("Speaker 1", "system", 0, 8_000, "POHA system audio check."),
            transcript_segment(
                "Me",
                "mic",
                2_000,
                6_000,
                "I am checking the microphone now",
            ),
        ];

        suppress_system_bleed_duplicates(&mut segments);

        assert_eq!(segments.len(), 2);
        assert!(
            segments
                .iter()
                .any(|segment| segment.text == "I am checking the microphone now")
        );
    }

    #[test]
    fn me_and_call_mode_uses_available_stems() {
        let capture = tempdir().expect("temp capture");
        std::fs::write(capture.path().join("audio_mic.wav"), b"mic").expect("write mic");
        std::fs::write(capture.path().join("audio_spk.wav"), b"spk").expect("write spk");

        let inputs =
            labeled_audio_inputs(capture.path(), None, SpeakerLabelMode::MeAndCall).unwrap();

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].speaker, "Me");
        assert_eq!(inputs[0].source, "mic");
        assert_eq!(inputs[1].speaker, "Call");
        assert_eq!(inputs[1].source, "system");
    }

    #[test]
    fn me_and_call_mode_prefers_healthy_processed_mic() {
        let capture = tempdir().expect("temp capture");
        write_constant_wav(&capture.path().join(RAW_MIC_FILE), 3, 0.4);
        write_constant_wav(&capture.path().join(PROCESSED_MIC_FILE), 3, 0.3);
        write_constant_wav(&capture.path().join(SYSTEM_AUDIO_FILE), 3, 0.0);

        let inputs =
            labeled_audio_inputs(capture.path(), None, SpeakerLabelMode::MeAndCall).unwrap();

        assert_eq!(inputs[0].speaker, "Me");
        assert_eq!(inputs[0].source, "mic");
        assert_eq!(inputs[0].cache_source, "mic_processed");
        assert_eq!(inputs[0].path, capture.path().join(PROCESSED_MIC_FILE));
        assert_eq!(inputs[0].selection, "processedMic");
    }

    #[test]
    fn me_and_call_mode_falls_back_when_processed_mic_is_misaligned() {
        let capture = tempdir().expect("temp capture");
        write_constant_wav(&capture.path().join(RAW_MIC_FILE), 3, 0.4);
        write_constant_wav(&capture.path().join(PROCESSED_MIC_FILE), 1, 0.3);
        write_constant_wav(&capture.path().join(SYSTEM_AUDIO_FILE), 3, 0.0);

        let inputs =
            labeled_audio_inputs(capture.path(), None, SpeakerLabelMode::MeAndCall).unwrap();

        assert_eq!(inputs[0].path, capture.path().join(RAW_MIC_FILE));
        assert_eq!(inputs[0].cache_source, "mic");
        assert_eq!(inputs[0].selection, "rawMicFallback");
        assert!(
            inputs[0]
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("duration"))
        );
    }

    #[test]
    fn me_and_call_mode_falls_back_when_processed_mic_suppresses_raw_only_speech() {
        let capture = tempdir().expect("temp capture");
        write_constant_wav(&capture.path().join(RAW_MIC_FILE), 3, 0.4);
        write_constant_wav(&capture.path().join(PROCESSED_MIC_FILE), 3, 0.0001);
        write_constant_wav(&capture.path().join(SYSTEM_AUDIO_FILE), 3, 0.0);

        let inputs =
            labeled_audio_inputs(capture.path(), None, SpeakerLabelMode::MeAndCall).unwrap();

        assert_eq!(inputs[0].path, capture.path().join(RAW_MIC_FILE));
        assert_eq!(inputs[0].selection, "rawMicFallback");
        assert!(
            inputs[0]
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("too quiet"))
        );
    }

    #[test]
    fn fast_mode_uses_single_mixed_audio() {
        let capture = tempdir().expect("temp capture");
        std::fs::write(capture.path().join("audio.mp3"), b"mixed").expect("write mixed");
        std::fs::write(capture.path().join("audio_mic.wav"), b"mic").expect("write mic");
        std::fs::write(capture.path().join("audio_spk.wav"), b"spk").expect("write spk");

        let inputs =
            labeled_audio_inputs(capture.path(), None, SpeakerLabelMode::FastMixed).unwrap();

        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].speaker, "Speaker");
        assert_eq!(inputs[0].source, "mixed");
        assert_eq!(inputs[0].path, capture.path().join("audio.mp3"));
    }

    #[test]
    fn local_model_validation_rejects_safetensors_only_repo() {
        let dir = tempdir().expect("temp dir");
        std::fs::write(dir.path().join("model.safetensors"), "x").expect("write safetensors");
        let error =
            validate_local_mlx_model_dir("test-model", dir.path()).expect_err("expected error");
        assert!(error.contains("no weights.npz"));
    }

    #[test]
    fn local_model_validation_accepts_weights_npz_repo() {
        let dir = tempdir().expect("temp dir");
        std::fs::write(dir.path().join("weights.npz"), "x").expect("write npz");
        let result = validate_local_mlx_model_dir("test-model", dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn local_model_validation_accepts_weights_safetensors_repo() {
        let dir = tempdir().expect("temp dir");
        std::fs::write(dir.path().join("weights.safetensors"), "x").expect("write safetensors");
        let result = validate_local_mlx_model_dir("test-model", dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn vad_chunking_detects_separated_speech_regions() {
        let sample_rate = 1_000;
        let mut frames = vec![0.0; 1_000];
        frames.extend(vec![0.45; 4_500]);
        frames.extend(vec![0.0; 1_500]);
        frames.extend(vec![0.45; 4_500]);
        frames.extend(vec![0.0; 1_000]);

        let chunks = detect_speech_chunks(&frames, sample_rate);

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].0 <= 1_000);
        assert!(chunks[0].1 >= 5_500);
        assert!(chunks[1].0 <= 7_000);
        assert!(chunks[1].1 >= 11_500);
    }

    #[test]
    fn partial_wav_reader_reads_finalized_i16_wav() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("audio_mic.wav");
        write_speech_test_wav(&path);

        let wav = read_partial_wav(&path)
            .expect("read partial wav")
            .expect("partial wav");

        assert_eq!(wav.spec.channels, 1);
        assert_eq!(wav.spec.sample_rate, 1_000);
        assert_eq!(wav.frames.len(), 12_500);
        assert!(matches!(wav.samples, PartialSamples::I16(_)));
    }

    #[test]
    fn live_pass_writes_partial_snapshot_and_chunk_statuses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let capture = tempdir().expect("temp capture");
        let output = tempdir().expect("temp output");
        let fake_bin = tempdir().expect("fake bin");
        let uvx = fake_bin.path().join("uvx");
        std::fs::write(
            &uvx,
            "#!/bin/sh\nprintf '%s\\n' 'mock mlx' '{\"text\":\"\",\"segments\":[{\"start\":0.0,\"end\":1.0,\"text\":\"live transcript\"}]}'\n",
        )
        .expect("write uvx");
        make_executable(&uvx);
        write_speech_test_wav(&capture.path().join("audio_mic.wav"));

        let _path_restore = PathRestore(std::env::var_os("PATH"));
        unsafe {
            std::env::set_var("PATH", fake_bin.path());
        }

        let input = ProcessInput {
            session_id: "session-live".to_string(),
            capture_dir: capture.path().to_path_buf(),
            output_dir: output.path().to_path_buf(),
            audio_path_from_event: None,
            settings: test_settings(output.path(), SpeakerLabelMode::MeAndCall),
        };
        let mut snapshot = new_live_chunk_snapshot();

        process_live_transcription_pass(&input, &mut snapshot, true).expect("live pass");

        assert_eq!(snapshot.chunks.len(), 2);
        assert!(
            snapshot
                .chunks
                .iter()
                .all(|entry| entry.status == ChunkStatus::Done)
        );
        assert!(output.path().join("transcript.live.json").exists());
        assert!(output.path().join("transcript.chunks.json").exists());

        let partial = std::fs::read_to_string(output.path().join("transcript.partial.md"))
            .expect("read partial transcript");
        assert!(partial.contains("Me: live transcript"));
    }

    #[test]
    fn chunk_offset_fills_text_only_segments() {
        let mut segments = vec![transcript_segment("Speaker", "mixed", 0, 0, "hello")];

        offset_chunk_segments(&mut segments, 12_000, 18_000);

        assert_eq!(segments[0].start_ms, 12_000);
        assert_eq!(segments[0].end_ms, 18_000);
    }

    #[test]
    fn diarization_labels_system_segments_by_first_speaker_appearance() {
        let mut segments = vec![
            transcript_segment("Call", "system", 1_000, 2_000, "first"),
            transcript_segment("Call", "system", 3_000, 4_000, "second"),
        ];
        let diarization_segments = vec![
            DiarizationSegment {
                speaker_id: "raw-b".to_string(),
                start_ms: 800,
                end_ms: 2_200,
            },
            DiarizationSegment {
                speaker_id: "raw-a".to_string(),
                start_ms: 2_900,
                end_ms: 4_100,
            },
        ];

        apply_system_diarization(&mut segments, &diarization_segments);

        assert_eq!(segments[0].speaker, "Speaker 1");
        assert_eq!(segments[0].speaker_id.as_deref(), Some("raw-b"));
        assert_eq!(segments[1].speaker, "Speaker 2");
        assert_eq!(segments[1].speaker_id.as_deref(), Some("raw-a"));
    }

    #[test]
    fn diarization_prefers_best_overlap() {
        let mut segments = vec![transcript_segment("Call", "system", 3_000, 7_000, "hello")];
        let diarization_segments = vec![
            DiarizationSegment {
                speaker_id: "raw-a".to_string(),
                start_ms: 0,
                end_ms: 4_000,
            },
            DiarizationSegment {
                speaker_id: "raw-b".to_string(),
                start_ms: 3_500,
                end_ms: 10_000,
            },
        ];

        apply_system_diarization(&mut segments, &diarization_segments);

        assert_eq!(segments[0].speaker_id.as_deref(), Some("raw-b"));
    }

    #[test]
    fn diarization_uses_nearest_segment_for_small_asr_gaps() {
        let mut segments = vec![transcript_segment("Call", "system", 5_000, 6_000, "hello")];
        let diarization_segments = vec![DiarizationSegment {
            speaker_id: "raw-a".to_string(),
            start_ms: 6_500,
            end_ms: 8_000,
        }];

        apply_system_diarization(&mut segments, &diarization_segments);

        assert_eq!(segments[0].speaker, "Speaker 1");
        assert_eq!(segments[0].speaker_id.as_deref(), Some("raw-a"));
    }

    #[test]
    fn diarization_snaps_full_stem_system_timestamp_after_leading_silence() {
        let mut segments = vec![transcript_segment(
            "Call",
            "system",
            0,
            15_300,
            "Poha system audio check.",
        )];
        let diarization_segments = vec![DiarizationSegment {
            speaker_id: "raw-a".to_string(),
            start_ms: 13_864,
            end_ms: 15_316,
        }];

        apply_system_diarization(&mut segments, &diarization_segments);

        assert_eq!(segments[0].speaker, "Speaker 1");
        assert_eq!(segments[0].speaker_id.as_deref(), Some("raw-a"));
        assert_eq!(segments[0].start_ms, 13_864);
        assert_eq!(segments[0].end_ms, 15_316);
    }

    #[test]
    fn diarization_does_not_relabel_mic_segments() {
        let mut segments = vec![transcript_segment("Me", "mic", 1_000, 2_000, "hello")];
        let diarization_segments = vec![DiarizationSegment {
            speaker_id: "raw-a".to_string(),
            start_ms: 500,
            end_ms: 2_500,
        }];

        apply_system_diarization(&mut segments, &diarization_segments);

        assert_eq!(segments[0].speaker, "Me");
        assert_eq!(segments[0].speaker_id, None);
    }

    #[test]
    fn empty_diarization_keeps_existing_call_label() {
        let mut segments = vec![transcript_segment("Call", "system", 1_000, 2_000, "hello")];

        apply_system_diarization(&mut segments, &[]);

        assert_eq!(segments[0].speaker, "Call");
        assert_eq!(segments[0].speaker_id, None);
    }

    #[test]
    fn read_diarization_segments_accepts_wrapped_payload() {
        let capture = tempdir().expect("temp capture");
        let output = tempdir().expect("temp output");
        std::fs::write(
            capture.path().join("diarization.json"),
            r#"{"segments":[{"speakerId":"speaker-a","startMs":1000,"endMs":2000}]}"#,
        )
        .expect("write diarization file");

        let segments = read_diarization_segments(capture.path(), output.path())
            .expect("read diarization segments");

        assert_eq!(
            segments,
            vec![DiarizationSegment {
                speaker_id: "speaker-a".to_string(),
                start_ms: 1_000,
                end_ms: 2_000,
            }]
        );
    }

    #[test]
    fn process_session_writes_chunked_diarized_transcript_with_mock_mlx() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let capture = tempdir().expect("temp capture");
        let output = tempdir().expect("temp output");
        let fake_bin = tempdir().expect("fake bin");
        let uvx = fake_bin.path().join("uvx");
        std::fs::write(
            &uvx,
            "#!/bin/sh\nprintf '%s\\n' 'mock mlx' '{\"text\":\"\",\"segments\":[{\"start\":0.0,\"end\":1.0,\"text\":\"mock transcript\"}]}'\n",
        )
        .expect("write uvx");
        make_executable(&uvx);

        write_speech_test_wav(&capture.path().join("audio_mic.wav"));
        write_speech_test_wav(&capture.path().join("audio_spk.wav"));
        std::fs::write(
            capture.path().join("diarization.json"),
            r#"{"segments":[{"speakerId":"speaker-a","startMs":0,"endMs":20000}]}"#,
        )
        .expect("write diarization file");

        let _path_restore = PathRestore(std::env::var_os("PATH"));
        unsafe {
            std::env::set_var("PATH", fake_bin.path());
        }

        let result = process_session(ProcessInput {
            session_id: "session-e2e".to_string(),
            capture_dir: capture.path().to_path_buf(),
            output_dir: output.path().to_path_buf(),
            audio_path_from_event: None,
            settings: test_settings(output.path(), SpeakerLabelMode::MeAndCall),
        })
        .expect("process session");

        let transcript_md =
            std::fs::read_to_string(&result.transcript_markdown_path).expect("read transcript md");
        assert!(transcript_md.contains("Me: mock transcript"));
        assert!(transcript_md.contains("Speaker 1: mock transcript"));
        assert!(output.path().join("transcript.partial.md").exists());

        let transcript_json =
            std::fs::read_to_string(&result.transcript_json_path).expect("read transcript json");
        assert!(transcript_json.contains(r#""speakerId": "speaker-a""#));

        let chunk_manifest = std::fs::read_to_string(output.path().join("transcript.chunks.json"))
            .expect("read chunk manifest");
        assert!(chunk_manifest.contains("energy-vad-wav-chunks"));
        assert!(chunk_manifest.contains("transcript_chunks/mic"));
        assert!(chunk_manifest.contains("transcript_chunks/system"));
    }

    #[test]
    fn process_session_reuses_live_done_chunks() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let capture = tempdir().expect("temp capture");
        let output = tempdir().expect("temp output");
        let fake_bin = tempdir().expect("fake bin");
        let uvx = fake_bin.path().join("uvx");
        std::fs::write(&uvx, "#!/bin/sh\nexit 12\n").expect("write uvx");
        make_executable(&uvx);

        let mic_path = capture.path().join("audio_mic.wav");
        write_speech_test_wav(&mic_path);
        let chunks = chunk_wav_for_transcription(
            &LabeledAudioInput {
                path: mic_path,
                speaker: "Me",
                source: "mic",
                cache_source: "mic",
                file_name: RAW_MIC_FILE,
                selection: "rawMic",
                fallback_reason: None,
            },
            output.path(),
        )
        .expect("chunk wav");

        let mut snapshot = new_live_chunk_snapshot();
        for (index, chunk) in chunks.into_iter().enumerate() {
            snapshot.chunks.push(LiveChunkEntry {
                speaker: "Me".to_string(),
                source: "mic".to_string(),
                path: chunk.path.to_string_lossy().into_owned(),
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                status: ChunkStatus::Done,
                error: None,
                segments: vec![transcript_segment(
                    "Me",
                    "mic",
                    chunk.start_ms,
                    chunk.end_ms,
                    &format!("cached live chunk {index}"),
                )],
            });
        }
        write_live_chunk_snapshot(output.path(), &snapshot).expect("write live snapshot");

        let _path_restore = PathRestore(std::env::var_os("PATH"));
        unsafe {
            std::env::set_var("PATH", fake_bin.path());
        }

        let result = process_session(ProcessInput {
            session_id: "session-reuse".to_string(),
            capture_dir: capture.path().to_path_buf(),
            output_dir: output.path().to_path_buf(),
            audio_path_from_event: None,
            settings: test_settings(output.path(), SpeakerLabelMode::MeAndCall),
        })
        .expect("process session");

        let transcript_md =
            std::fs::read_to_string(&result.transcript_markdown_path).expect("read transcript md");
        assert!(transcript_md.contains("cached live chunk 0"));
        assert!(transcript_md.contains("cached live chunk 1"));
    }

    #[test]
    fn prepare_diarization_segments_skips_when_system_audio_is_missing() {
        let capture = tempdir().expect("temp capture");
        let output = tempdir().expect("temp output");

        let segments = prepare_diarization_segments(capture.path(), output.path())
            .expect("prepare diarization");

        assert!(segments.is_empty());
    }

    #[test]
    fn diarizer_binary_candidates_include_env_exe_and_bundled_paths() {
        let manifest = tempdir().expect("temp manifest");
        let exe = tempdir().expect("temp exe");
        let env_path = PathBuf::from("/tmp/custom-poha-diarizer");

        let candidates = diarizer_binary_candidates_for(
            Some(env_path.clone()),
            Some(exe.path().to_path_buf()),
            manifest.path(),
            "aarch64-apple-darwin",
        );

        assert_eq!(candidates[0], env_path);
        assert!(candidates.contains(&exe.path().join("poha-diarizer")));
        assert!(candidates.contains(&exe.path().join("poha-diarizer-aarch64-apple-darwin")));
        assert!(
            candidates.contains(
                &manifest
                    .path()
                    .join("binaries")
                    .join("poha-diarizer-aarch64-apple-darwin")
            )
        );
    }

    struct PathRestore(Option<std::ffi::OsString>);

    impl Drop for PathRestore {
        fn drop(&mut self) {
            unsafe {
                match self.0.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn write_speech_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 1_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        let mut samples = vec![0_i16; 1_000];
        samples.extend(vec![16_000; 4_500]);
        samples.extend(vec![0; 1_500]);
        samples.extend(vec![16_000; 4_500]);
        samples.extend(vec![0; 1_000]);

        for sample in samples {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }

    fn write_constant_wav(path: &Path, seconds: usize, amplitude: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 1_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        let sample = (amplitude.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        for _ in 0..seconds * 1_000 {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }

    fn test_settings(
        recordings_dir: &Path,
        speaker_label_mode: SpeakerLabelMode,
    ) -> RecorderSettings {
        RecorderSettings {
            recordings_dir: recordings_dir.to_string_lossy().into_owned(),
            mlx_model: "mock-model".to_string(),
            mlx_fallback_model: "mock-model".to_string(),
            mic_device_id: None,
            preserve_stems: false,
            speaker_label_mode,
            system_audio_authorized_hint: true,
            onboarding_completed: true,
            meeting_end_reminders_enabled: true,
            meeting_automation_mode: crate::meeting_detection::AutomationMode::Off,
            calendar_integration_enabled: false,
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("set executable");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
