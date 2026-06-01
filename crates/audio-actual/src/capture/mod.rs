mod joiner;
mod stream;

use poha_audio::{CaptureConfig, CaptureStream, Error};
use stream::{CaptureSide, setup_mic_stream, setup_speaker_stream};

pub(crate) fn open_capture(config: CaptureConfig) -> Result<CaptureStream, Error> {
    let mic_stream = setup_mic_stream(config.sample_rate, config.chunk_size, config.mic_device)?;

    std::thread::sleep(std::time::Duration::from_millis(50));

    let speaker_stream = setup_speaker_stream(config.sample_rate, config.chunk_size)?;

    Ok(stream::open_dual(
        config.sample_rate,
        mic_stream,
        speaker_stream,
        config.enable_aec,
    ))
}

pub(crate) fn open_speaker_capture(
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    let speaker_stream = setup_speaker_stream(sample_rate, chunk_size)?;
    Ok(stream::open_single(speaker_stream, CaptureSide::Speaker))
}

pub(crate) fn open_mic_capture(
    device: Option<String>,
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    let mic_stream = setup_mic_stream(sample_rate, chunk_size, device)?;
    Ok(stream::open_single(mic_stream, CaptureSide::Mic))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use futures_util::StreamExt;
    use poha_audio_utils::chunk_size_for_stt;

    use super::*;

    #[tokio::test]
    #[ignore = "requires microphone and system audio permissions"]
    async fn dual_capture_emits_mic_frames_when_speaker_is_silent() {
        let sample_rate = 16_000;
        let mut stream = open_capture(CaptureConfig {
            sample_rate,
            chunk_size: chunk_size_for_stt(sample_rate),
            mic_device: Some("MacBook Pro Microphone".to_string()),
            enable_aec: false,
        })
        .expect("dual capture opens");

        let mut received = None;
        for _ in 0..10 {
            if let Ok(Some(frame)) =
                tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await
            {
                received = Some(frame.expect("capture frame"));
                break;
            }
        }

        let frame = received.expect("dual capture frame");
        assert!(
            frame.raw_mic.iter().any(|sample| *sample != 0.0),
            "mic side should contain captured samples"
        );
    }
}
