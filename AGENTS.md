# Overview

Poha is a Tauri macOS menu bar meeting recorder. Active app: `apps/poha/`.

The app records locally, writes session artifacts to disk, runs chunked transcription, and exposes `poha-cli` for agent/automation reads.

## Commands

- Install: `pnpm install`
- Run app: `./script/build_and_run.sh`
- Format: `pnpm exec dprint fmt`
- Rust check: `pnpm cargo:check`
- CLI tests: `pnpm test:cli`
- Library tests: `pnpm test:lib`
- Transcription tests: `pnpm test:transcription`
- Build app: `pnpm poha:build`

## Guidelines

- Keep the repo focused on Poha. Do not reintroduce unrelated apps, web surfaces, cloud services, or old CI without explicit need.
- Format with dprint after edits.
- Run `pnpm cargo:check` after Rust changes.
- Run focused regression tests when touching CLI, sessions, recording, transcription, or tray behavior.
- Keep files under roughly 500 LOC when changing them substantially; split only when it reduces real complexity.
- Use Conventional Commits.
- Push only when asked.

## Session Contract

- Session artifacts live under a recordings directory in `sessions/<session-id>/`.
- Important files: `session.json`, `meeting.json`, `summary.md`, `transcript.md`, `transcript.partial.md`, `transcript.json`, `transcript.live.json`, `transcript.chunks.json`, `.poha/meetings.sqlite`.
- `poha-cli` should emit stable JSON.
- Agent-safe CLI writes are `summary.md`, `meeting.json`, `.poha/meetings.sqlite`, and `.poha/exports`; never rewrite audio, session JSON, or transcript evidence.
