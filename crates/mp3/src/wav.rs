use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;

use hound::SampleFormat;

use crate::encoder::{f32_to_i16, int_to_i16};
use crate::{Error, MonoStreamEncoder, StereoStreamEncoder};

const CHUNK_FRAMES: usize = 4096;
const TARGET_STEREO_CHANNEL_GAP_DB: f64 = 6.0;
const MAX_STEREO_CHANNEL_BOOST_DB: f64 = 14.0;
const MIN_ACTIVE_CHANNEL_DBFS: f64 = -60.0;
const LIMITER_CEILING: f32 = 0.98;

pub fn concat_files(paths: &[&Path], output: &Path) -> Result<(), Error> {
    let file = File::create(output)?;
    let mut writer = BufWriter::new(file);

    for path in paths {
        let bytes = std::fs::read(path)?;
        writer.write_all(&bytes)?;
    }

    writer.flush()?;
    Ok(())
}

pub fn encode_wav(wav_path: &Path, mp3_path: &Path) -> Result<(), Error> {
    encode_wav_inner(wav_path, mp3_path, false)
}

pub fn encode_wav_mastered(wav_path: &Path, mp3_path: &Path) -> Result<(), Error> {
    encode_wav_inner(wav_path, mp3_path, true)
}

fn encode_wav_inner(wav_path: &Path, mp3_path: &Path, master_stereo: bool) -> Result<(), Error> {
    let mut reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();
    let mut mp3_out = Vec::new();

    match (spec.channels, master_stereo) {
        (1, _) => encode_mono_wav(&mut reader, spec, &mut mp3_out)?,
        (2, true) => {
            drop(reader);
            encode_stereo_wav_mastered(wav_path, spec, &mut mp3_out)?;
        }
        (2, false) => encode_stereo_wav(&mut reader, spec, &mut mp3_out)?,
        (count, _) => return Err(Error::UnsupportedChannelCount(count)),
    }

    std::fs::write(mp3_path, &mp3_out)?;
    Ok(())
}

pub fn decode_to_wav(mp3_path: &Path, wav_path: &Path) -> Result<(), Error> {
    use poha_audio_utils::Source;

    let source = poha_audio_utils::source_from_path(mp3_path)?;
    let channels: u16 = source.channels().into();
    let sample_rate: u32 = source.sample_rate().into();

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(wav_path, spec)?;
    for sample in source {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}

fn encode_mono_wav<R: std::io::Read + Seek>(
    reader: &mut hound::WavReader<R>,
    spec: hound::WavSpec,
    mp3_out: &mut Vec<u8>,
) -> Result<(), Error> {
    let mut encoder = MonoStreamEncoder::new(spec.sample_rate)?;

    match spec.sample_format {
        SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(Error::UnsupportedFloatBitDepth(spec.bits_per_sample));
            }

            encode_mono_samples(reader.samples::<f32>(), f32_to_i16, |chunk| {
                encoder.encode_i16(chunk, mp3_out)
            })?;
        }
        SampleFormat::Int => match spec.bits_per_sample {
            1..=8 => encode_mono_samples(
                reader.samples::<i8>(),
                |sample| int_to_i16(sample as i32, spec.bits_per_sample),
                |chunk| encoder.encode_i16(chunk, mp3_out),
            )?,
            9..=16 => encode_mono_samples(
                reader.samples::<i16>(),
                |sample| int_to_i16(sample as i32, spec.bits_per_sample),
                |chunk| encoder.encode_i16(chunk, mp3_out),
            )?,
            17..=32 => encode_mono_samples(
                reader.samples::<i32>(),
                |sample| int_to_i16(sample, spec.bits_per_sample),
                |chunk| encoder.encode_i16(chunk, mp3_out),
            )?,
            bits => return Err(Error::UnsupportedIntBitDepth(bits)),
        },
    }

    encoder.flush(mp3_out)?;
    Ok(())
}

fn encode_stereo_wav<R: std::io::Read + Seek>(
    reader: &mut hound::WavReader<R>,
    spec: hound::WavSpec,
    mp3_out: &mut Vec<u8>,
) -> Result<(), Error> {
    let mut encoder = StereoStreamEncoder::new(spec.sample_rate)?;

    match spec.sample_format {
        SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(Error::UnsupportedFloatBitDepth(spec.bits_per_sample));
            }

            encode_stereo_samples(reader.samples::<f32>(), f32_to_i16, |left, right| {
                encoder.encode_i16(left, right, mp3_out)
            })?;
        }
        SampleFormat::Int => match spec.bits_per_sample {
            1..=8 => encode_stereo_samples(
                reader.samples::<i8>(),
                |sample| int_to_i16(sample as i32, spec.bits_per_sample),
                |left, right| encoder.encode_i16(left, right, mp3_out),
            )?,
            9..=16 => encode_stereo_samples(
                reader.samples::<i16>(),
                |sample| int_to_i16(sample as i32, spec.bits_per_sample),
                |left, right| encoder.encode_i16(left, right, mp3_out),
            )?,
            17..=32 => encode_stereo_samples(
                reader.samples::<i32>(),
                |sample| int_to_i16(sample, spec.bits_per_sample),
                |left, right| encoder.encode_i16(left, right, mp3_out),
            )?,
            bits => return Err(Error::UnsupportedIntBitDepth(bits)),
        },
    }

    encoder.flush(mp3_out)?;
    Ok(())
}

fn encode_stereo_wav_mastered(
    wav_path: &Path,
    spec: hound::WavSpec,
    mp3_out: &mut Vec<u8>,
) -> Result<(), Error> {
    let stats = stereo_stats_for_wav(wav_path, spec)?;
    let plan = stereo_mastering_plan(&stats);
    let mut reader = hound::WavReader::open(wav_path)?;
    let mut encoder = StereoStreamEncoder::new(spec.sample_rate)?;

    match spec.sample_format {
        SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(Error::UnsupportedFloatBitDepth(spec.bits_per_sample));
            }

            encode_stereo_samples_f32(
                reader.samples::<f32>(),
                |sample| sample,
                plan,
                |left, right| encoder.encode_f32(left, right, mp3_out),
            )?;
        }
        SampleFormat::Int => match spec.bits_per_sample {
            1..=8 => encode_stereo_samples_f32(
                reader.samples::<i8>(),
                |sample| int_to_f32(sample as i32, spec.bits_per_sample),
                plan,
                |left, right| encoder.encode_f32(left, right, mp3_out),
            )?,
            9..=16 => encode_stereo_samples_f32(
                reader.samples::<i16>(),
                |sample| int_to_f32(sample as i32, spec.bits_per_sample),
                plan,
                |left, right| encoder.encode_f32(left, right, mp3_out),
            )?,
            17..=32 => encode_stereo_samples_f32(
                reader.samples::<i32>(),
                |sample| int_to_f32(sample, spec.bits_per_sample),
                plan,
                |left, right| encoder.encode_f32(left, right, mp3_out),
            )?,
            bits => return Err(Error::UnsupportedIntBitDepth(bits)),
        },
    }

    encoder.flush(mp3_out)?;
    Ok(())
}

fn stereo_stats_for_wav(wav_path: &Path, spec: hound::WavSpec) -> Result<StereoStats, Error> {
    let mut reader = hound::WavReader::open(wav_path)?;
    match spec.sample_format {
        SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(Error::UnsupportedFloatBitDepth(spec.bits_per_sample));
            }
            analyze_stereo_samples(reader.samples::<f32>(), |sample| sample)
        }
        SampleFormat::Int => match spec.bits_per_sample {
            1..=8 => analyze_stereo_samples(reader.samples::<i8>(), |sample| {
                int_to_f32(sample as i32, spec.bits_per_sample)
            }),
            9..=16 => analyze_stereo_samples(reader.samples::<i16>(), |sample| {
                int_to_f32(sample as i32, spec.bits_per_sample)
            }),
            17..=32 => analyze_stereo_samples(reader.samples::<i32>(), |sample| {
                int_to_f32(sample, spec.bits_per_sample)
            }),
            bits => Err(Error::UnsupportedIntBitDepth(bits)),
        },
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StereoStats {
    left_sum_squares: f64,
    right_sum_squares: f64,
    left_peak: f32,
    right_peak: f32,
    frames: u64,
}

impl StereoStats {
    fn observe(&mut self, left: f32, right: f32) {
        if left.is_finite() {
            self.left_sum_squares += f64::from(left) * f64::from(left);
            self.left_peak = self.left_peak.max(left.abs());
        }
        if right.is_finite() {
            self.right_sum_squares += f64::from(right) * f64::from(right);
            self.right_peak = self.right_peak.max(right.abs());
        }
        self.frames = self.frames.saturating_add(1);
    }

    fn left_rms_dbfs(&self) -> Option<f64> {
        rms_dbfs(self.left_sum_squares, self.frames)
    }

    fn right_rms_dbfs(&self) -> Option<f64> {
        rms_dbfs(self.right_sum_squares, self.frames)
    }
}

#[derive(Debug, Clone, Copy)]
struct StereoMasteringPlan {
    left_gain: f32,
    right_gain: f32,
}

impl Default for StereoMasteringPlan {
    fn default() -> Self {
        Self {
            left_gain: 1.0,
            right_gain: 1.0,
        }
    }
}

fn analyze_stereo_samples<S, I, F>(
    mut samples: I,
    mut sample_to_f32: F,
) -> Result<StereoStats, Error>
where
    I: Iterator<Item = Result<S, hound::Error>>,
    F: FnMut(S) -> f32,
{
    let mut stats = StereoStats::default();
    loop {
        let Some(left_sample) = samples.next() else {
            break;
        };
        let left = sample_to_f32(left_sample?);
        let right = match samples.next() {
            Some(right_sample) => sample_to_f32(right_sample?),
            None => 0.0,
        };
        stats.observe(left, right);
    }
    Ok(stats)
}

fn stereo_mastering_plan(stats: &StereoStats) -> StereoMasteringPlan {
    let Some(left_rms) = stats.left_rms_dbfs() else {
        return StereoMasteringPlan::default();
    };
    let Some(right_rms) = stats.right_rms_dbfs() else {
        return StereoMasteringPlan::default();
    };
    if !left_rms.is_finite() || !right_rms.is_finite() {
        return StereoMasteringPlan::default();
    }
    if left_rms < MIN_ACTIVE_CHANNEL_DBFS || right_rms < MIN_ACTIVE_CHANNEL_DBFS {
        return StereoMasteringPlan::default();
    }

    let gap = (left_rms - right_rms).abs();
    if gap <= TARGET_STEREO_CHANNEL_GAP_DB {
        return StereoMasteringPlan::default();
    }

    let boost_db = (gap - TARGET_STEREO_CHANNEL_GAP_DB).min(MAX_STEREO_CHANNEL_BOOST_DB);
    let boost = db_to_gain(boost_db) as f32;
    if left_rms < right_rms {
        StereoMasteringPlan {
            left_gain: boost,
            right_gain: 1.0,
        }
    } else {
        StereoMasteringPlan {
            left_gain: 1.0,
            right_gain: boost,
        }
    }
}

fn encode_stereo_samples_f32<S, I, F, E>(
    mut samples: I,
    mut sample_to_f32: F,
    plan: StereoMasteringPlan,
    mut encode_chunk: E,
) -> Result<(), Error>
where
    I: Iterator<Item = Result<S, hound::Error>>,
    F: FnMut(S) -> f32,
    E: FnMut(&[f32], &[f32]) -> Result<(), Error>,
{
    let mut left = Vec::with_capacity(CHUNK_FRAMES);
    let mut right = Vec::with_capacity(CHUNK_FRAMES);

    loop {
        let Some(left_sample) = samples.next() else {
            break;
        };
        left.push(apply_mastering_sample(
            sample_to_f32(left_sample?),
            plan.left_gain,
        ));

        let right_sample = match samples.next() {
            Some(right_sample) => sample_to_f32(right_sample?),
            None => 0.0,
        };
        right.push(apply_mastering_sample(right_sample, plan.right_gain));

        if left.len() < CHUNK_FRAMES {
            continue;
        }

        encode_chunk(&left, &right)?;
        left.clear();
        right.clear();
    }

    if !left.is_empty() {
        encode_chunk(&left, &right)?;
    }

    Ok(())
}

fn apply_mastering_sample(sample: f32, gain: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    (sample * gain).clamp(-LIMITER_CEILING, LIMITER_CEILING)
}

fn encode_mono_samples<S, I, F, E>(
    samples: I,
    mut sample_to_i16: F,
    mut encode_chunk: E,
) -> Result<(), Error>
where
    I: Iterator<Item = Result<S, hound::Error>>,
    F: FnMut(S) -> i16,
    E: FnMut(&[i16]) -> Result<(), Error>,
{
    let mut pcm_i16 = Vec::with_capacity(CHUNK_FRAMES);
    for sample in samples {
        pcm_i16.push(sample_to_i16(sample?));
        if pcm_i16.len() < CHUNK_FRAMES {
            continue;
        }

        encode_chunk(&pcm_i16)?;
        pcm_i16.clear();
    }

    if !pcm_i16.is_empty() {
        encode_chunk(&pcm_i16)?;
    }

    Ok(())
}

fn encode_stereo_samples<S, I, F, E>(
    mut samples: I,
    mut sample_to_i16: F,
    mut encode_chunk: E,
) -> Result<(), Error>
where
    I: Iterator<Item = Result<S, hound::Error>>,
    F: FnMut(S) -> i16,
    E: FnMut(&[i16], &[i16]) -> Result<(), Error>,
{
    let mut left = Vec::with_capacity(CHUNK_FRAMES);
    let mut right = Vec::with_capacity(CHUNK_FRAMES);

    loop {
        let Some(left_sample) = samples.next() else {
            break;
        };
        left.push(sample_to_i16(left_sample?));

        match samples.next() {
            Some(right_sample) => right.push(sample_to_i16(right_sample?)),
            None => right.push(0i16),
        }

        if left.len() < CHUNK_FRAMES {
            continue;
        }

        encode_chunk(&left, &right)?;
        left.clear();
        right.clear();
    }

    if !left.is_empty() {
        encode_chunk(&left, &right)?;
    }

    Ok(())
}

fn int_to_f32(sample: i32, bits_per_sample: u16) -> f32 {
    let max_amplitude = match bits_per_sample {
        0 | 1 => return 0.0,
        32.. => i32::MAX as f32,
        bits => ((1i64 << (bits - 1)) - 1) as f32,
    };
    (sample as f32 / max_amplitude).clamp(-1.0, 1.0)
}

fn rms_dbfs(sum_squares: f64, count: u64) -> Option<f64> {
    if count == 0 {
        return None;
    }
    let rms = (sum_squares / count as f64).sqrt();
    (rms > 0.0).then(|| 20.0 * rms.log10())
}

fn db_to_gain(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encode_mono_samples_flushes_partial_tail() -> Result<(), Error> {
        let samples = (0..(CHUNK_FRAMES + 1))
            .map(|n| Ok(n as i16))
            .collect::<Vec<_>>()
            .into_iter();
        let mut chunk_sizes = Vec::new();

        encode_mono_samples(
            samples,
            |sample| sample,
            |chunk| {
                chunk_sizes.push(chunk.len());
                Ok(())
            },
        )?;

        assert_eq!(chunk_sizes, vec![CHUNK_FRAMES, 1]);
        Ok(())
    }

    #[test]
    fn encode_stereo_samples_pads_missing_right_sample() -> Result<(), Error> {
        let samples = vec![Ok(10i16), Ok(20i16), Ok(30i16)].into_iter();
        let mut encoded = Vec::new();

        encode_stereo_samples(
            samples,
            |sample| sample,
            |left, right| {
                encoded.push((left.to_vec(), right.to_vec()));
                Ok(())
            },
        )?;

        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].0, vec![10, 30]);
        assert_eq!(encoded[0].1, vec![20, 0]);
        Ok(())
    }

    #[test]
    fn concat_files_joins_bytes_in_order() -> Result<(), Error> {
        let dir = tempdir()?;
        let first = dir.path().join("a.mp3");
        let second = dir.path().join("b.mp3");
        let output = dir.path().join("out.mp3");

        std::fs::write(&first, [1u8, 2, 3])?;
        std::fs::write(&second, [4u8, 5, 6])?;

        concat_files(&[&first, &second], &output)?;

        assert_eq!(std::fs::read(output)?, vec![1, 2, 3, 4, 5, 6]);
        Ok(())
    }

    #[test]
    fn stereo_mastering_plan_boosts_quieter_channel_toward_target_gap() {
        let mut stats = StereoStats::default();
        for _ in 0..16_000 {
            stats.observe(0.08, 0.4);
        }

        let plan = stereo_mastering_plan(&stats);

        assert!(plan.left_gain > 1.0);
        assert_eq!(plan.right_gain, 1.0);

        let mastered_left = 0.08 * plan.left_gain;
        let gap_db = 20.0 * f64::from(0.4 / mastered_left).log10();
        assert!(gap_db <= TARGET_STEREO_CHANNEL_GAP_DB + 0.1);
    }

    #[test]
    fn apply_mastering_sample_limits_boosted_peaks() {
        assert_eq!(apply_mastering_sample(1.0, 2.0), LIMITER_CEILING);
        assert_eq!(apply_mastering_sample(-1.0, 2.0), -LIMITER_CEILING);
    }
}
