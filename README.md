# Poha

Poha is a macOS menu bar meeting recorder. It records locally, writes session files to disk, transcribes in speech-boundary chunks, and exposes an agent-safe CLI for reading session artifacts.

## Layout

- `apps/poha/` - Tauri menu bar app.
- `apps/poha/src-tauri/src/bin/poha-cli.rs` - JSON CLI for agents and automations.
- `crates/` and `plugins/` - local Rust crates required by Poha.

## Run

```sh
pnpm install
./script/build_and_run.sh
```

The script starts `pnpm -F poha tauri:dev`.

## Build And Check

```sh
pnpm exec dprint fmt
pnpm cargo:check
pnpm test:cli
pnpm test:lib
pnpm test:transcription
pnpm poha:build
./script/run_live_test.sh
```

`pnpm poha:build` produces a macOS bundle with stable bundle id `com.iamadig.poha` and the microphone entitlement required for the packaged app. If no codesigning identity is installed, Tauri signs ad-hoc; after an ad-hoc rebuild, macOS may require re-granting Microphone and System Audio before the packaged app can pass live testing. Set `APPLE_SIGNING_IDENTITY` to an installed codesigning identity for a more stable local permission identity.

## Privacy Defaults

Poha records and transcribes locally. The standalone app does not configure a remote crash-reporting DSN, and analytics are inactive unless a build explicitly compiles in a valid `POSTHOG_API_KEY`.

Release builds should document any enabled network analytics, model downloads, or cloud transcription settings before distribution.

## CLI

Read when: using Poha from agents, scripts, or local automations.

```sh
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- spec
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- capabilities
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- sessions list
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- sessions get <session-id> --partial
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- sessions status <session-id>
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- storage report
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- storage maintain --dry-run
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- diagnostics audio --guided
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- meetings list
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- meetings get <meeting-id> --include metadata,transcript,paths
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- meetings update <meeting-id> --title "New title"
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- meetings speakers <meeting-id> --set "Speaker 1=Adi"
cargo run --manifest-path apps/poha/src-tauri/Cargo.toml --bin poha-cli -- meetings reindex
```

Use `POHA_RECORDINGS_DIR` or `--recordings-dir` to point the CLI at a specific recordings folder.

## Session Files

Read when: changing recording, transcription, meeting browser, meeting metadata, or CLI write behavior.

Each recording session is file-backed. Important outputs include:

- `session.json`
- `meeting.json`
- `summary.md`
- `transcript.md`
- `transcript.partial.md`
- `transcript.json`
- `transcript.live.json`
- `transcript.chunks.json`
- `.poha/meetings.sqlite`

## Storage Lifecycle

Read when: changing recording finalization, transcription chunking, session deletion, or storage CLI behavior.

Poha runs default storage maintenance automatically after a session reaches `done` and once at app startup. Maintenance never hard-deletes files. It moves eligible artifacts to macOS Trash.

New recordings preserve raw mic audio in the mixed source recording and `audio_mic.wav`. When AEC is available, Poha also writes `audio_mic_processed.wav` as a transcription candidate. Final `Me` transcription uses the processed mic only after decode, duration, sample-rate, and raw-only speech checks pass; otherwise it falls back to raw `audio_mic.wav`. The raw stem remains the evidence/debug source. System transcript timestamps can be snapped to system diarization when full-stem ASR places a short playback phrase at the start of the file.

Final stereo `audio.mp3` uses listening-oriented mastering: Poha adaptively lifts the quieter channel toward the other channel and applies a peak limiter while leaving raw stems untouched.

Default retention:

- `transcript_chunks/` is trashed after `transcript.md` and valid `transcript.json` exist, chunk manifests have no queued/transcribing/failed chunks, and mixed `audio.mp3` is valid. If chunk manifests are absent, the cleanup ledger records `absentWithFinalTranscript`.
- `audio_mic.wav`, `audio_mic_processed.wav`, and `audio_spk.wav` are trashed only after the same transcript validation, valid mixed `audio.mp3`, duration plausibility checks, a mixed MP3 mic/system loudness gate, and a last-playable-source invariant. Stem cleanup waits for a later pass if `audio.mp3` still needs to be created from legacy WAV.
- Legacy `audio.wav` is encoded to `audio.mp3`, validated, duration-checked, then moved to Trash. If `audio.mp3` already validates, only the legacy WAV is trashed.
- `.poha/deleted/<session>` is moved to Trash after 30 days.

Use `poha-cli storage maintain --dry-run` to inspect planned maintenance. The CLI dry-run does not move files.
Use `poha-cli storage audio-quality --limit 200` to find non-active sessions whose mixed `audio.mp3` likely has a missing, silent, too-quiet, or stem-degraded mic channel. The default audit samples the opening 30 seconds for speed; add `--full` for a complete decode. The audit is read-only and separates source availability (`canRegenerateMp3`) from whether regeneration is likely to fix the MP3 (`repairableByRegeneration`).

The tray keeps diagnostics under `Troubleshooting`. `Run Quick Live Test` records through the normal mic+system capture path, plays a short local diagnostic phrase, waits for audio frames before stopping, and writes `live-test-report.json` plus `live-test-report.md`. `Run Guided Audio Check` opens a small floating Audio Check window, counts down, prompts the user to say "Poha microphone check", then plays "Poha system audio check" from system audio. The guided report validates raw mic, processed mic, final MP3 channel balance, system phase, relative level, transcript phrase match, transcript timing, capture timebase, and storage lifecycle expectations. Permission failures write a preflight report under `.poha/live-tests/`; completed capture tests write the report into the session folder.

`poha-cli diagnostics audio --guided` triggers the same guided check through the running menu bar app. It does not duplicate recording logic in the CLI.

Use `./script/run_live_test.sh` for an end-to-end packaged-app check. It builds Poha, verifies bundle id and entitlements, launches the packaged app, reads tray permission state, triggers `Run Live Test`, waits for `live-test-report.json`, then prints the JSON and Markdown report paths. It requires an unlocked desktop session because it drives the menu bar tray. If it exits with `Screen is locked`, unlock the Mac and rerun it. If packaged-app permissions are missing, it asks Poha to write a preflight report and tells you to re-grant Poha under System Settings.

Actual maintenance appends `.poha/storage-maintenance.jsonl` with moved paths, byte counts, reasons, and validation facts. If an artifact from that ledger is restored, future maintenance skips it instead of moving it back to Trash.

The CLI never rewrites source evidence. Agent-safe writes are limited to `summary.md`, `meeting.json`, `.poha/meetings.sqlite`, and `.poha/exports`.
