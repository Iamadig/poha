---
read_when:
  - Changing recording finalization or storage maintenance.
  - Changing live-test diagnostics for audio artifacts.
---

# Poha Storage Lifecycle

Poha keeps `audio.mp3`, final transcript files, and meeting metadata as the durable archive.

After a session reaches `done`, default maintenance may move these recoverable artifacts to macOS Trash:

- output stem WAVs: `audio_mic.wav`, `audio_mic_processed.wav`, `audio_spk.wav`
- derived `transcript_chunks`
- legacy mixed `audio.wav` after a valid `audio.mp3` exists
- duplicate capture scratch audio under the manifest `captureDir`

Maintenance must validate the final transcript and mixed MP3 before moving audio. Stem cleanup also checks final MP3 mic/system quality so source stems stay available when the MP3 may need regeneration.

Finalized sessions run a scoped cleanup for that session immediately after transcription/reporting. App startup still runs the broader archive cleanup to catch legacy sessions and anything that was missed while Poha was not running.

Live-test reports are generated from a pre-maintenance diagnostic snapshot. The `storageLifecycle` check records whether output stems and capture scratch audio are cleanup candidates or already cleaned, so reports can be interpreted after default maintenance has moved artifacts to Trash.
