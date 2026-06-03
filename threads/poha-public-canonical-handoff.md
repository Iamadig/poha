# poha-public-canonical-handoff — Thread
Status: OPEN
Ball: Claude (adi)
Topic: poha-public-canonical-handoff
Created: 2026-06-03T20:02:46.372736+00:00

---
Entry: Codex (adi) 2026-06-03T20:02:46.372736+00:00
Role: implementer
Type: Note
Title: Poha public repo is canonical

Context for future Codex work on Poha.

Canonical repo/workspace:
- Use `/Users/adi/projects/poha-public` for all Poha development.
- GitHub remote: `https://github.com/Iamadig/poha.git`.
- Private repo `/Users/adi/projects/poha` points to `Iamadig/poha-dev`; keep frozen as backup only.

Migration status:
- Fresh public repo has been created from the cleaned Poha tree and is now the source of truth.
- `THIRD_PARTY_NOTICES.md` was simplified and pushed in commit `303b087 docs: simplify third-party notices`.
- Public `main` was clean and pushed when last checked.
- Private dirty branch/source files were compared against public; active source/doc content matched public. No important code remained to copy.
- No GitHub issues/tags/releases needed migration from `poha-dev`; labels and CI matched.

Installed app status:
- Rebuilt from `/Users/adi/projects/poha-public` with `pnpm poha:build`.
- Installed app is `/Applications/Poha.app` and was launched from there.
- Installed app binary hash matched the freshly built public bundle.
- Installed CLI successfully read existing recordings in `/Users/adi/Library/Application Support/Poha/recordings`.
- Private target app/DMG were trashed to avoid confusion:
  - `/Users/adi/projects/poha/target/release/bundle/macos/Poha.app`
  - `/Users/adi/projects/poha/target/release/bundle/dmg/Poha_0.1.0_aarch64.dmg`
- Public build artifacts currently exist:
  - `/Users/adi/projects/poha-public/target/release/bundle/macos/Poha.app`
  - `/Users/adi/projects/poha-public/target/release/bundle/dmg/Poha_0.1.0_aarch64.dmg`

Dogfood/open-source readiness:
- Public repo dogfood was done with an isolated staging bundle and temp recordings dir.
- Tested launch, browser, search, company filters, summary/transcript/details, metadata edit/save, copy actions, export all, re-scan, Codex enrichment scoped to temp recordings, short recording start/stop, mic/system capture, transcription completion, CLI indexing/list/get.
- Recommendation from that audit: public source release is ready; notarized binary distribution is separate and not done.
- Known non-blocking warning: repeated log warning `microphone permission probe succeeded after AVFoundation reported missing`; functionality worked and permissions showed green.

Future build/install flow:
```sh
cd ~/projects/poha-public
git pull --ff-only
pnpm install
pnpm poha:build
trash /Applications/Poha.app
cp -R ~/projects/poha-public/target/release/bundle/macos/Poha.app /Applications/
open /Applications/Poha.app
```

User preference:
- User wants to continue Poha work from the public repo and stop using private `poha-dev` except as a temporary backup.
- Keep replies terse and direct.

<!-- Entry-ID: 01KT7H9GV1Z270A1QJNXX10Z71 -->
