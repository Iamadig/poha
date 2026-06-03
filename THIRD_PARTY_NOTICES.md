# Third-Party Notices

Poha includes a few bundled models and media assets. This file is intentionally lightweight; package dependency licenses remain with their upstream crates and packages.

## Bundled Models

- DTLN-aec AEC models and sample inputs: MIT, <https://github.com/breizhn/DTLN-aec>.
- DTLN denoise models: MIT, <https://github.com/breizhn/DTLN>.
- Silero VAD model: MIT, <https://github.com/snakers4/silero-vad>.
- Pyannote segmentation ONNX model: MIT, <https://huggingface.co/onnx-community/pyannote-segmentation-3.0>.
- Speaker embedding ONNX model: <https://huggingface.co/csukuangfj/speaker-embedding-models>. Upstream WeSpeaker notes that VoxCeleb-trained models follow the VoxCeleb dataset terms.

## App Assets And Fixtures

Poha also includes app icons, tray icons, UI sounds, and local test audio fixtures. Treat these as part of the Poha source tree unless an individual asset is replaced with a clearly external asset; if that happens, add the source and license here.

## Downloaded Models

Whisper, Llama, and Gemma model files are not bundled in this repository. If users download them through Poha or manually place them on disk, their original model licenses apply.
