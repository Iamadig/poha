use std::fs;
use std::path::Path;

use serde_json::Value;

use super::*;

#[test]
fn report_wrapper_emits_storage_report_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = dir.path().join("session-1");
    fs::create_dir_all(&session).expect("session dir");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"done"}"#,
    )
    .expect("session json");
    fs::copy(
        poha_data::english_1::AUDIO_MP3_PATH,
        session.join("audio.mp3"),
    )
    .expect("mp3");

    let ctx = Context {
        recordings_dir: dir.path().to_path_buf(),
    };
    let value = serde_json::to_value(report(&ctx, 10).expect("report")).expect("json");

    assert_eq!(value["totals"]["activeRecordingsCount"], Value::from(1));
    assert_eq!(value["totals"]["mixedSourceAudioFileCount"], Value::from(1));
}

#[test]
fn audio_quality_wrapper_reports_affected_mp3s() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = dir.path().join("session-1");
    fs::create_dir_all(&session).expect("session dir");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"done"}"#,
    )
    .expect("session json");
    write_stereo_mp3(&session.join("audio.mp3"), 1, 0.002, 0.6).expect("mp3");
    write_mono_wav(&session.join("audio_mic.wav"), 1, 0.4).expect("mic");
    write_mono_wav(&session.join("audio_spk.wav"), 1, 0.35).expect("spk");

    let ctx = Context {
        recordings_dir: dir.path().to_path_buf(),
    };
    let value = serde_json::to_value(audio_quality(&ctx, 10, false, false).expect("quality"))
        .expect("json");

    assert_eq!(value["analysisWindowSeconds"], Value::from(30));
    assert_eq!(value["totals"]["affectedSessionCount"], Value::from(1));
    assert_eq!(value["sessions"][0]["status"], Value::from("micTooQuiet"));
    assert_eq!(
        value["sessions"][0]["recommendation"],
        Value::from("regenerateMp3")
    );
    assert_eq!(
        value["sessions"][0]["repairableByRegeneration"],
        Value::from(true)
    );
}

#[test]
fn maintain_dry_run_wrapper_returns_plan() {
    let dir = tempfile::tempdir().expect("temp dir");
    let session = dir.path().join("session-1");
    fs::create_dir_all(session.join("transcript_chunks").join("mic")).expect("chunks dir");
    fs::write(
        session.join("session.json"),
        r#"{"id":"session-1","status":"done"}"#,
    )
    .expect("session json");
    fs::write(session.join("transcript.md"), "hello").expect("transcript md");
    fs::write(session.join("transcript.json"), r#"{"segments":[]}"#).expect("transcript json");
    fs::copy(
        poha_data::english_1::AUDIO_MP3_PATH,
        session.join("audio.mp3"),
    )
    .expect("mp3");
    fs::write(
        session
            .join("transcript_chunks")
            .join("mic")
            .join("mic-0000.wav"),
        [1u8; 10],
    )
    .expect("chunk");

    let ctx = Context {
        recordings_dir: dir.path().to_path_buf(),
    };
    let value = serde_json::to_value(maintain_dry_run(&ctx).expect("plan")).expect("json");

    assert_eq!(value["dryRun"], Value::from(true));
    assert_eq!(value["totals"]["candidateCount"], Value::from(1));
    assert_eq!(value["actions"][0]["kind"], Value::from("transcriptChunks"));
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
