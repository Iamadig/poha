use std::fs;
use std::path::Path;

use chrono::{TimeZone, Utc};

use super::*;

struct FakeTrash {
    root: PathBuf,
}

impl TrashSink for FakeTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), StorageError> {
        fs::create_dir_all(&self.root)
            .map_err(|error| StorageError::new("testTrashFailed", error.to_string()))?;
        let name = path
            .file_name()
            .ok_or_else(|| StorageError::new("testTrashFailed", "missing file name"))?;
        fs::rename(path, self.root.join(name))
            .map_err(|error| StorageError::new("testTrashFailed", error.to_string()))
    }
}

#[test]
fn report_splits_stems_chunks_and_legacy_wavs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = dir.path().join("session-1");
    fs::create_dir_all(session.join("transcript_chunks").join("mic")).expect("chunks dir");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"done"}"#,
    )
    .expect("session json");
    fs::write(session.join("audio.mp3"), [1u8; 10]).expect("mp3");
    fs::write(session.join("audio.wav"), [2u8; 20]).expect("legacy wav");
    fs::write(session.join("audio_mic.wav"), [3u8; 30]).expect("mic");
    fs::write(session.join("audio_spk.wav"), [4u8; 40]).expect("spk");
    fs::write(
        session
            .join("transcript_chunks")
            .join("mic")
            .join("mic-0000.wav"),
        [5u8; 50],
    )
    .expect("chunk");

    let report = serde_json::to_value(report(dir.path(), 10).expect("report")).expect("json");

    assert_eq!(report["totals"]["mixedSourceAudioBytes"], 10);
    assert_eq!(report["totals"]["legacyWavSourceAudioBytes"], 20);
    assert_eq!(report["totals"]["stemWavBytes"], 70);
    assert_eq!(report["totals"]["derivedChunkAudioBytes"], 50);
    assert!(
        report["reclaimable"]
            .as_array()
            .expect("reclaimable array")
            .iter()
            .any(|candidate| candidate["kind"] == "stemWavs" && candidate["bytes"] == 70)
    );
}

#[test]
fn plan_cleans_finalized_chunks_and_stems_only_after_validation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session(dir.path(), "session-1");
    fs::create_dir_all(session.join("transcript_chunks").join("mic")).expect("chunks dir");
    fs::write(
        session
            .join("transcript_chunks")
            .join("mic")
            .join("mic-0000.wav"),
        [1u8; 10],
    )
    .expect("chunk");
    fs::write(
        session.join("transcript.chunks.json"),
        r#"{"chunks":[{"status":"done"}]}"#,
    )
    .expect("chunks json");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");
    let kinds = plan
        .actions
        .iter()
        .map(|action| action.kind.as_str())
        .collect::<Vec<_>>();

    assert!(kinds.contains(&"transcriptChunks"));
    assert!(kinds.contains(&"stemWav"));
}

#[test]
fn plan_preserves_session_stems_but_still_cleans_derived_chunks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session(dir.path(), "session-1");
    write_session_json_with_policy(&session, "session-1", true);
    fs::create_dir_all(session.join("transcript_chunks")).expect("chunks dir");
    fs::write(
        session.join("transcript_chunks").join("chunk-0000.wav"),
        [1u8; 10],
    )
    .expect("chunk");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");
    write_mono_wav(&session.join("audio_spk.wav"), 1, 0.35).expect("system");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");

    assert!(plan.actions.iter().all(|action| action.kind != "stemWav"));
    assert!(
        plan.actions
            .iter()
            .any(|action| action.kind == "transcriptChunks")
    );
    assert_eq!(
        plan.skipped
            .iter()
            .filter(|skipped| skipped.reason.contains("preserveStems"))
            .count(),
        2
    );
}

#[test]
fn plan_skips_source_wav_when_duration_mismatches_mixed_mp3() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session(dir.path(), "session-1");
    write_silent_wav(&session.join("audio_mic.wav"), 30).expect("silent wav");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");

    assert!(plan.actions.iter().all(|action| action.kind != "stemWav"));
    assert!(
        plan.skipped
            .iter()
            .any(|skipped| skipped.reason.contains("mixed MP3 duration"))
    );
}

#[test]
fn plan_uses_source_wav_duration_over_noisy_session_duration() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session(dir.path(), "session-1");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"done","startedAt":"2026-05-30T10:00:00Z","endedAt":"2026-05-30T12:00:00Z"}"#,
    )
    .expect("session json");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");
    fs::create_dir_all(session.join("transcript_chunks")).expect("chunks dir");
    fs::write(
        session.join("transcript_chunks").join("chunk-0000.wav"),
        [1u8; 10],
    )
    .expect("chunk");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");
    let value = serde_json::to_value(&plan).expect("plan json");
    let stem = value["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .find(|action| action["kind"] == "stemWav")
        .expect("stem action");

    assert!(
        stem["validationFacts"]["durationChecks"]
            .as_array()
            .expect("duration checks")
            .iter()
            .any(|check| {
                check["source"] == "session" && check["status"] == "outsideToleranceAuditOnly"
            })
    );
    assert!(
        value["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|action| action["kind"] == "transcriptChunks"),
        "{value}"
    );
}

#[test]
fn plan_keeps_stems_when_mixed_mp3_mic_is_too_quiet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.002, 0.6).expect("mp3");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");

    assert!(plan.actions.iter().all(|action| action.kind != "stemWav"));
    assert!(
        plan.skipped
            .iter()
            .any(|skipped| skipped.reason.contains("audio quality check failed")),
        "{:?}",
        plan.skipped
    );
}

#[test]
fn maintain_trashes_capture_scratch_audio_after_final_archive_validation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let capture = tempfile::tempdir().expect("capture");
    let session = final_session(dir.path(), "session-1");
    write_session_json_with_capture(&session, "session-1", capture.path());
    write_mono_wav(&capture.path().join("audio_mic.wav"), 1, 0.4).expect("capture mic");

    let result = maintain_with_trash(
        dir.path(),
        Utc::now(),
        &FakeTrash {
            root: trash.path().to_path_buf(),
        },
    )
    .expect("maintain");

    assert!(
        result
            .moved_to_trash
            .iter()
            .any(|action| action.kind == "captureScratchAudio")
    );
    assert!(!capture.path().join("audio_mic.wav").exists());
    assert!(trash.path().join("audio_mic.wav").exists());
}

#[test]
fn maintain_never_trashes_output_or_capture_stems_when_policy_preserves_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let capture = tempfile::tempdir().expect("capture");
    let session = final_session(dir.path(), "session-1");
    write_session_json_with_capture_and_policy(&session, "session-1", capture.path(), true);
    for stem in ["audio_mic.wav", "audio_mic_processed.wav", "audio_spk.wav"] {
        write_mono_wav(&session.join(stem), 1, 0.4).expect("output stem");
        write_mono_wav(&capture.path().join(stem), 1, 0.4).expect("capture stem");
    }

    let result = maintain_with_trash(
        dir.path(),
        Utc::now(),
        &FakeTrash {
            root: trash.path().to_path_buf(),
        },
    )
    .expect("maintain");

    assert!(result.moved_to_trash.is_empty());
    for stem in ["audio_mic.wav", "audio_mic_processed.wav", "audio_spk.wav"] {
        assert!(session.join(stem).exists(), "missing output {stem}");
        assert!(capture.path().join(stem).exists(), "missing capture {stem}");
        assert!(!trash.path().join(stem).exists(), "trashed {stem}");
    }
}

#[test]
fn maintain_session_only_sweeps_requested_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let capture_one = tempfile::tempdir().expect("capture one");
    let capture_two = tempfile::tempdir().expect("capture two");
    let session_one = final_session(dir.path(), "session-1");
    let session_two = final_session(dir.path(), "session-2");
    write_session_json_with_capture(&session_one, "session-1", capture_one.path());
    write_session_json_with_capture(&session_two, "session-2", capture_two.path());
    write_mono_wav(&capture_one.path().join("audio_mic.wav"), 1, 0.4).expect("capture one mic");
    write_mono_wav(&capture_two.path().join("audio_mic.wav"), 1, 0.4).expect("capture two mic");

    let result = maintain_session_with_trash(
        dir.path(),
        &session_one,
        Utc::now(),
        &FakeTrash {
            root: trash.path().to_path_buf(),
        },
    )
    .expect("maintain session");

    assert_eq!(result.moved_to_trash.len(), 1);
    assert!(!capture_one.path().join("audio_mic.wav").exists());
    assert!(capture_two.path().join("audio_mic.wav").exists());
    assert!(
        result
            .moved_to_trash
            .iter()
            .all(|action| action.session_id.as_deref() == Some("session-1"))
    );
}

#[test]
fn plan_keeps_capture_scratch_stem_when_mixed_mp3_mic_is_too_quiet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let capture = tempfile::tempdir().expect("capture");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_session_json_with_capture(&session, "session-1", capture.path());
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.002, 0.6).expect("mp3");
    write_mono_wav(&capture.path().join("audio_mic.wav"), 1, 0.4).expect("capture mic");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");

    assert!(
        plan.actions
            .iter()
            .all(|action| action.kind != "captureScratchAudio")
    );
    assert!(
        plan.skipped
            .iter()
            .any(|skipped| skipped.reason.contains("audio quality check failed")),
        "{:?}",
        plan.skipped
    );
}

#[test]
fn audio_quality_report_flags_quiet_mp3_that_can_be_regenerated() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.002, 0.6).expect("mp3");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");
    write_mono_wav(&session.join("audio_spk.wav"), 1, 0.35).expect("spk");

    let report = serde_json::to_value(audio_quality_report(dir.path(), 10, false).expect("report"))
        .expect("json");

    assert_eq!(report["totals"]["affectedSessionCount"], 1);
    assert_eq!(report["totals"]["regenerateMp3SessionCount"], 1);
    assert_eq!(report["sessions"][0]["status"], "micTooQuiet");
    assert_eq!(report["sessions"][0]["recommendation"], "regenerateMp3");
    assert_eq!(report["sessions"][0]["canRegenerateMp3"], true);
    assert_eq!(report["sessions"][0]["repairableByRegeneration"], true);
}

#[test]
fn audio_quality_report_reviews_quiet_mp3_when_stem_is_also_quiet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.002, 0.6).expect("mp3");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.002).expect("mic");
    write_mono_wav(&session.join("audio_spk.wav"), 1, 0.6).expect("spk");

    let report = serde_json::to_value(audio_quality_report(dir.path(), 10, false).expect("report"))
        .expect("json");

    assert_eq!(report["totals"]["affectedSessionCount"], 1);
    assert_eq!(report["totals"]["reviewSessionCount"], 1);
    assert_eq!(report["sessions"][0]["status"], "micTooQuiet");
    assert_eq!(report["sessions"][0]["recommendation"], "review");
    assert_eq!(report["sessions"][0]["canRegenerateMp3"], true);
    assert_eq!(report["sessions"][0]["repairableByRegeneration"], false);
}

#[test]
fn audio_quality_report_scans_recorded_sessions_with_audio() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session_without_audio(dir.path(), "session-1");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"recorded"}"#,
    )
    .expect("session json");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.4, 0.35).expect("mp3");

    let report = serde_json::to_value(audio_quality_report(dir.path(), 10, false).expect("report"))
        .expect("json");

    assert_eq!(report["totals"]["scannedSessionCount"], 1);
    assert_eq!(report["totals"]["okSessionCount"], 1);
}

#[test]
fn audio_quality_report_flags_mp3_mic_degraded_from_restored_stem() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.1, 0.03).expect("mp3");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.6).expect("mic");
    write_mono_wav(&session.join("audio_spk.wav"), 1, 0.03).expect("spk");

    let report = serde_json::to_value(audio_quality_report(dir.path(), 10, false).expect("report"))
        .expect("json");

    assert_eq!(report["totals"]["affectedSessionCount"], 1);
    assert_eq!(report["sessions"][0]["status"], "micDegradedFromStem");
    assert_eq!(report["sessions"][0]["recommendation"], "regenerateMp3");
    assert_eq!(report["sessions"][0]["repairableByRegeneration"], true);
}

#[test]
fn plan_skips_unfinished_chunk_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = final_session(dir.path(), "session-1");
    fs::create_dir_all(session.join("transcript_chunks")).expect("chunks dir");
    fs::write(
        session.join("transcript.chunks.json"),
        r#"{"chunks":[{"status":"failed"}]}"#,
    )
    .expect("chunks json");

    let plan = maintenance_plan(dir.path(), Utc::now()).expect("plan");

    assert!(plan.actions.is_empty());
}

#[test]
fn maintain_transcodes_legacy_wav_before_trashing_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let session = final_session_without_audio(dir.path(), "session-1");
    write_mono_wav(&session.join("audio.wav"), 1, 0.4).expect("wav");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("stem");

    let result = maintain_with_trash(
        dir.path(),
        Utc::now(),
        &FakeTrash {
            root: trash.path().to_path_buf(),
        },
    )
    .expect("maintain");

    assert_eq!(result.moved_to_trash.len(), 1);
    assert!(session.join("audio.mp3").exists());
    assert!(!session.join("audio.wav").exists());
    assert!(session.join("audio_mic.wav").exists());
    assert!(trash.path().join("audio.wav").exists());
    assert!(!trash.path().join("audio_mic.wav").exists());
}

#[test]
fn maintain_writes_storage_maintenance_ledger() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let session = final_session(dir.path(), "session-1");
    fs::create_dir_all(session.join("transcript_chunks")).expect("chunks dir");
    fs::write(
        session.join("transcript_chunks").join("chunk-0000.wav"),
        [1u8; 10],
    )
    .expect("chunk");

    let result = maintain_with_trash(
        dir.path(),
        Utc::now(),
        &FakeTrash {
            root: trash.path().to_path_buf(),
        },
    )
    .expect("maintain");
    let ledger =
        fs::read_to_string(dir.path().join(".poha/storage-maintenance.jsonl")).expect("ledger");
    let entry: serde_json::Value = serde_json::from_str(ledger.trim()).expect("ledger json");

    assert_eq!(result.moved_to_trash.len(), 1);
    assert_eq!(entry["kind"], "transcriptChunks");
    assert_eq!(
        entry["validationFacts"]["chunkManifestState"],
        "absentWithFinalTranscript"
    );
}

#[test]
fn plan_trashes_deleted_sessions_after_retention_window() {
    let dir = tempfile::tempdir().expect("temp dir");
    let deleted = dir.path().join(".poha").join("deleted");
    fs::create_dir_all(deleted.join("20260401-000000-old")).expect("old");
    fs::create_dir_all(deleted.join("20260525-000000-new")).expect("new");
    let now = Utc.with_ymd_and_hms(2026, 5, 30, 0, 0, 0).unwrap();

    let plan = maintenance_plan(dir.path(), now).expect("plan");

    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].kind, "deletedRecording");
    assert!(plan.actions[0].path.contains("20260401-000000-old"));
}

fn final_session(root: &Path, id: &str) -> PathBuf {
    let session = final_session_without_audio(root, id);
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.4, 0.35).expect("mp3");
    session
}

fn final_session_without_audio(root: &Path, id: &str) -> PathBuf {
    let session = root.join(id);
    fs::create_dir_all(&session).expect("session dir");
    fs::write(
        session.join("session.json"),
        format!(r#"{{"id":"{id}","status":"done"}}"#),
    )
    .expect("session json");
    fs::write(session.join("transcript.md"), "hello").expect("transcript md");
    fs::write(session.join("transcript.json"), r#"{"segments":[]}"#).expect("transcript json");
    session
}

fn write_session_json_with_capture(session: &Path, id: &str, capture_dir: &Path) {
    fs::write(
        session.join("session.json"),
        format!(
            r#"{{"id":"{id}","status":"done","captureDir":"{}"}}"#,
            capture_dir.to_string_lossy()
        ),
    )
    .expect("session json");
}

fn write_session_json_with_policy(session: &Path, id: &str, preserve_stems: bool) {
    fs::write(
        session.join("session.json"),
        format!(r#"{{"id":"{id}","status":"done","preserveStems":{preserve_stems}}}"#),
    )
    .expect("session json");
}

fn write_session_json_with_capture_and_policy(
    session: &Path,
    id: &str,
    capture_dir: &Path,
    preserve_stems: bool,
) {
    fs::write(
        session.join("session.json"),
        format!(
            r#"{{"id":"{id}","status":"done","captureDir":"{}","preserveStems":{preserve_stems}}}"#,
            capture_dir.to_string_lossy()
        ),
    )
    .expect("session json");
}

fn write_silent_wav(path: &Path, duration_secs: u32) -> Result<(), hound::Error> {
    write_mono_wav(path, duration_secs, 0.0)
}

fn write_mono_wav(path: &Path, duration_secs: u32, amplitude: f32) -> Result<(), hound::Error> {
    let sample_rate = 8_000;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for _ in 0..sample_rate * duration_secs {
        writer.write_sample(amplitude)?;
    }
    writer.finalize()
}

fn write_stereo_mp3(
    path: &Path,
    duration_secs: u32,
    mic_amplitude: f32,
    system_amplitude: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let wav_path = path.with_extension("source.wav");
    let sample_rate = 8_000;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&wav_path, spec)?;
    for _ in 0..sample_rate * duration_secs {
        writer.write_sample(mic_amplitude)?;
        writer.write_sample(system_amplitude)?;
    }
    writer.finalize()?;
    poha_mp3::encode_wav(&wav_path, path)?;
    fs::remove_file(wav_path)?;
    Ok(())
}
