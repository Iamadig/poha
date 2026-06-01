use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Instant;

use poha_audio_utils::{
    decode_vorbis_to_mono_wav_file, decode_vorbis_to_wav_file, mix_audio_f32,
    ogg_has_identical_channels,
};
use ractor::ActorProcessingErr;

use super::into_actor_err;

const FINAL_AUDIO_FILE: &str = "audio.mp3";
const WAV_FILE: &str = "audio.wav";
const OGG_FILE: &str = "audio.ogg";
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000);

pub(super) struct DiskSink {
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    writer_mic: Option<hound::WavWriter<BufWriter<File>>>,
    writer_mic_processed: Option<hound::WavWriter<BufWriter<File>>>,
    writer_spk: Option<hound::WavWriter<BufWriter<File>>>,
    mic_processed_path: Option<PathBuf>,
    pending_processed_mic_samples: usize,
    wav_path: PathBuf,
    last_flush: Instant,
    is_stereo: bool,
    mono_spec: hound::WavSpec,
}

pub(super) fn create_disk_sink(session_dir: &Path) -> Result<DiskSink, ActorProcessingErr> {
    let wav_path = session_dir.join(WAV_FILE);
    let ogg_path = session_dir.join(OGG_FILE);
    let encoded_path = session_dir.join(FINAL_AUDIO_FILE);
    let is_stereo = prepare_existing_audio_state(&encoded_path, &ogg_path, &wav_path)?;

    let stereo_spec = hound::WavSpec {
        channels: 2,
        sample_rate: super::super::SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mono_spec = hound::WavSpec {
        channels: 1,
        sample_rate: super::super::SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let writer = if wav_path.exists() {
        hound::WavWriter::append(&wav_path)?
    } else if is_stereo {
        hound::WavWriter::create(&wav_path, stereo_spec)?
    } else {
        hound::WavWriter::create(&wav_path, mono_spec)?
    };

    let stems_enabled = is_debug_mode();
    let (writer_mic, writer_mic_processed, writer_spk, mic_processed_path) = if stems_enabled {
        let mic_path = session_dir.join("audio_mic.wav");
        let mic_processed_path = session_dir.join("audio_mic_processed.wav");
        let spk_path = session_dir.join("audio_spk.wav");

        let mic_writer = open_mono_writer(&mic_path, mono_spec)?;
        let mic_processed_writer = if mic_processed_path.exists() {
            Some(hound::WavWriter::append(&mic_processed_path)?)
        } else {
            None
        };
        let spk_writer = open_mono_writer(&spk_path, mono_spec)?;

        (
            Some(mic_writer),
            mic_processed_writer,
            Some(spk_writer),
            Some(mic_processed_path),
        )
    } else {
        (None, None, None, None)
    };

    Ok(DiskSink {
        writer: Some(writer),
        writer_mic,
        writer_mic_processed,
        writer_spk,
        mic_processed_path,
        pending_processed_mic_samples: 0,
        wav_path,
        last_flush: Instant::now(),
        is_stereo,
        mono_spec,
    })
}

pub(super) fn write_single(sink: &mut DiskSink, samples: &[f32]) -> Result<(), ActorProcessingErr> {
    if let Some(writer) = sink.writer.as_mut() {
        if sink.is_stereo {
            write_mono_as_stereo(writer, samples)?;
        } else {
            write_mono_samples(writer, samples)?;
        }
    }

    flush_if_due(sink)?;
    Ok(())
}

pub(super) fn write_dual(
    sink: &mut DiskSink,
    raw_mic: &[f32],
    processed_mic: Option<&[f32]>,
    spk: &[f32],
) -> Result<(), ActorProcessingErr> {
    if let Some(writer) = sink.writer.as_mut() {
        if sink.is_stereo {
            write_interleaved_stereo(writer, raw_mic, spk)?;
        } else {
            let mixed = mix_audio_f32(raw_mic, spk);
            write_mono_samples(writer, &mixed)?;
        }
    }

    if let Some(writer_mic) = sink.writer_mic.as_mut() {
        write_mono_samples(writer_mic, raw_mic)?;
    }

    write_processed_mic(sink, raw_mic.len(), processed_mic)?;

    if let Some(writer_spk) = sink.writer_spk.as_mut() {
        write_mono_samples(writer_spk, spk)?;
    }

    flush_if_due(sink)?;
    Ok(())
}

pub(super) fn finalize_disk_sink(sink: &mut DiskSink) -> Result<(), ActorProcessingErr> {
    finalize_writer(&mut sink.writer, Some(&sink.wav_path))?;
    finalize_writer(&mut sink.writer_mic, None)?;
    finalize_writer(&mut sink.writer_mic_processed, None)?;
    finalize_writer(&mut sink.writer_spk, None)?;

    if sink.wav_path.exists() {
        let encoded_path = sink.wav_path.with_extension("mp3");
        match poha_mp3::encode_wav_mastered(&sink.wav_path, &encoded_path) {
            Ok(()) => {
                sync_file(&encoded_path);
                sync_dir(&encoded_path);
                std::fs::remove_file(&sink.wav_path)?;
                sync_dir(&sink.wav_path);
            }
            Err(error) => {
                tracing::error!("Encoding to mp3 failed, keeping WAV: {}", error);
                sync_file(&sink.wav_path);
                sync_dir(&sink.wav_path);
            }
        }
    }

    Ok(())
}

fn open_mono_writer(
    path: &Path,
    spec: hound::WavSpec,
) -> Result<hound::WavWriter<BufWriter<File>>, hound::Error> {
    if path.exists() {
        hound::WavWriter::append(path)
    } else {
        hound::WavWriter::create(path, spec)
    }
}

fn write_processed_mic(
    sink: &mut DiskSink,
    raw_sample_count: usize,
    processed_mic: Option<&[f32]>,
) -> Result<(), hound::Error> {
    let Some(samples) = processed_mic else {
        if let Some(writer) = sink.writer_mic_processed.as_mut() {
            write_silence(writer, raw_sample_count)?;
        } else if sink.mic_processed_path.is_some() {
            sink.pending_processed_mic_samples = sink
                .pending_processed_mic_samples
                .saturating_add(raw_sample_count);
        }
        return Ok(());
    };

    if sink.writer_mic_processed.is_none()
        && let Some(path) = sink.mic_processed_path.as_ref()
    {
        sink.writer_mic_processed = Some(open_mono_writer(path, sink.mono_spec)?);
    }

    if let Some(writer) = sink.writer_mic_processed.as_mut() {
        if sink.pending_processed_mic_samples > 0 {
            write_silence(writer, sink.pending_processed_mic_samples)?;
            sink.pending_processed_mic_samples = 0;
        }
        write_mono_samples(writer, samples)?;
    }

    Ok(())
}

fn prepare_existing_audio_state(
    encoded_path: &Path,
    ogg_path: &Path,
    wav_path: &Path,
) -> Result<bool, ActorProcessingErr> {
    if encoded_path.exists() && !wav_path.exists() {
        poha_mp3::decode_to_wav(encoded_path, wav_path).map_err(into_actor_err)?;
        std::fs::remove_file(encoded_path)?;
    }

    if ogg_path.exists() {
        let has_identical = ogg_has_identical_channels(ogg_path).map_err(into_actor_err)?;
        if has_identical {
            decode_vorbis_to_mono_wav_file(ogg_path, wav_path).map_err(into_actor_err)?;
        } else {
            decode_vorbis_to_wav_file(ogg_path, wav_path).map_err(into_actor_err)?;
        }
        std::fs::remove_file(ogg_path)?;
        return Ok(!has_identical);
    }

    if wav_path.exists() {
        let reader = hound::WavReader::open(wav_path)?;
        return Ok(reader.spec().channels == 2);
    }

    Ok(true)
}

fn is_debug_mode() -> bool {
    cfg!(debug_assertions)
        || cfg!(feature = "always-stem-audio")
        || std::env::var("LISTENER_DEBUG")
            .map(|value| !value.is_empty() && value != "0" && value != "false")
            .unwrap_or(false)
}

fn flush_if_due(sink: &mut DiskSink) -> Result<(), hound::Error> {
    if sink.last_flush.elapsed() < FLUSH_INTERVAL {
        return Ok(());
    }

    flush_all(sink)
}

fn flush_all(sink: &mut DiskSink) -> Result<(), hound::Error> {
    if let Some(writer) = sink.writer.as_mut() {
        writer.flush()?;
    }
    if let Some(writer_mic) = sink.writer_mic.as_mut() {
        writer_mic.flush()?;
    }
    if let Some(writer_mic_processed) = sink.writer_mic_processed.as_mut() {
        writer_mic_processed.flush()?;
    }
    if let Some(writer_spk) = sink.writer_spk.as_mut() {
        writer_spk.flush()?;
    }
    sink.last_flush = Instant::now();
    Ok(())
}

fn write_mono_samples(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    samples: &[f32],
) -> Result<(), hound::Error> {
    for sample in samples {
        writer.write_sample(*sample)?;
    }
    Ok(())
}

fn write_silence(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    sample_count: usize,
) -> Result<(), hound::Error> {
    for _ in 0..sample_count {
        writer.write_sample(0.0_f32)?;
    }
    Ok(())
}

fn write_mono_as_stereo(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    samples: &[f32],
) -> Result<(), hound::Error> {
    for sample in samples {
        writer.write_sample(*sample)?;
        writer.write_sample(*sample)?;
    }
    Ok(())
}

fn write_interleaved_stereo(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    mic: &[f32],
    spk: &[f32],
) -> Result<(), hound::Error> {
    let frames = mic.len().max(spk.len());
    for i in 0..frames {
        writer.write_sample(mic.get(i).copied().unwrap_or(0.0))?;
        writer.write_sample(spk.get(i).copied().unwrap_or(0.0))?;
    }
    Ok(())
}

fn finalize_writer(
    writer: &mut Option<hound::WavWriter<BufWriter<File>>>,
    path: Option<&Path>,
) -> Result<(), hound::Error> {
    if let Some(mut writer) = writer.take() {
        writer.flush()?;
        writer.finalize()?;

        if let Some(path) = path {
            sync_file(path);
        }
    }
    Ok(())
}

fn sync_file(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

fn sync_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn create_disk_sink_decodes_existing_mp3_to_wav() {
        let dir = tempdir().unwrap();
        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::copy(
            poha_data::english_1::AUDIO_MP3_PATH,
            session_dir.join(FINAL_AUDIO_FILE),
        )
        .unwrap();

        let _sink = create_disk_sink(&session_dir).unwrap();

        assert!(session_dir.join(WAV_FILE).exists());
        assert!(!session_dir.join(FINAL_AUDIO_FILE).exists());
    }

    #[test]
    fn create_disk_sink_keeps_legacy_wav_for_append() {
        let dir = tempdir().unwrap();
        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::copy(poha_data::english_1::AUDIO_PATH, session_dir.join(WAV_FILE)).unwrap();

        let _sink = create_disk_sink(&session_dir).unwrap();

        assert!(session_dir.join(WAV_FILE).exists());
        assert!(!session_dir.join(FINAL_AUDIO_FILE).exists());
    }

    #[test]
    fn write_dual_records_raw_and_processed_mic_as_source_audio() {
        let dir = tempdir().unwrap();
        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut sink = create_disk_sink(&session_dir).unwrap();

        write_dual(&mut sink, &[0.5, -0.5], Some(&[0.25, -0.25]), &[0.2, -0.2]).unwrap();
        finalize_writer(&mut sink.writer, Some(&sink.wav_path)).unwrap();
        finalize_writer(&mut sink.writer_mic, None).unwrap();
        finalize_writer(&mut sink.writer_mic_processed, None).unwrap();
        finalize_writer(&mut sink.writer_spk, None).unwrap();

        let mut mixed_reader = hound::WavReader::open(session_dir.join(WAV_FILE)).unwrap();
        let mixed = mixed_reader
            .samples::<f32>()
            .take(4)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(mixed, vec![0.5, 0.2, -0.5, -0.2]);

        let mut mic_reader = hound::WavReader::open(session_dir.join("audio_mic.wav")).unwrap();
        let mic = mic_reader
            .samples::<f32>()
            .take(2)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(mic, vec![0.5, -0.5]);

        let mut processed_reader =
            hound::WavReader::open(session_dir.join("audio_mic_processed.wav")).unwrap();
        let processed = processed_reader
            .samples::<f32>()
            .take(2)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(processed, vec![0.25, -0.25]);
    }

    #[test]
    fn write_dual_preserves_timebase_when_processed_mic_starts_late() {
        let dir = tempdir().unwrap();
        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut sink = create_disk_sink(&session_dir).unwrap();

        write_dual(&mut sink, &[0.5, -0.5], None, &[0.2, -0.2]).unwrap();
        write_dual(&mut sink, &[0.1, -0.1], Some(&[0.3, -0.3]), &[0.4, -0.4]).unwrap();
        finalize_writer(&mut sink.writer_mic_processed, None).unwrap();

        let mut reader =
            hound::WavReader::open(session_dir.join("audio_mic_processed.wav")).unwrap();
        let samples = reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(samples, vec![0.0, 0.0, 0.3, -0.3]);
    }
}
