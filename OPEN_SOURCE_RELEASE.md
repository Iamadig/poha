# Open Source Release Plan

Poha is not ready to make public by changing the current GitHub repository visibility.

The safe release path is a clean public repository created from the cleaned current tree, not the existing private Git history.

## Current Audit Evidence

- Current tree direct legacy-name scan passes for the old project and product terms tracked in the release audit.
- Current tracked filename scan passes for those same legacy terms.
- Current repo history fails the public-release gate. The private repo still has 7,176 commits and historical paths for old apps, API, website, billing, internal integrations, importer, and token/keygen areas.
- Current repo history size is about 617 MB.
- A throwaway one-commit public-history repo created from the current tree passed old path/name scans and secret scans. Its `.git` size is still about 161 MB because large assets remain in the tree.
- Large bundled assets still need provenance and license decisions before release. Main areas: `crates/data`, `crates/aec/data`, `crates/pyannote-local/src/data`, `crates/denoise/data`, and `crates/vad/data`. Unused `crates/data` text-only fixtures `english_4` through `english_9` were removed, including a large real-meeting transcript bundle; unreferenced transcript/provider JSON fixtures were also removed.
- Analytics code exists through PostHog dependencies, but network analytics are inactive unless a valid `POSTHOG_API_KEY` is compiled in. The standalone app does not initialize a remote crash-reporting DSN.

## Hard Blockers

- Publish from a fresh public history. The current repository was extracted from a larger private workspace and still has old history. Create a clean orphan branch or a new public repository from the current working tree after cleanup.
- Run secret scans on the final public tree and the final public history. Use at least `gitleaks` and `trufflehog`; do this after the clean-history import, not before.
- Remove, replace, or fully license large raw fixtures before publishing. Current candidates include checked-in audio files, transcript JSON, JSONL transcription fixtures, and bundled ONNX models.
- Decide replacement distribution for AM and Cactus STT model archives. Those downloads are disabled in the open-source tree until a public source or Poha-owned hosting is chosen.
- Keep the internal crate namespace on Poha-facing aliases. The Rust workspace dependency aliases now use `poha-*` / `poha_`.
- Keep telemetry default-off for public builds. Any build that compiles in analytics or initializes crash reporting needs a matching disclosure before release.

## Required Files

- `LICENSE`
- `README.md`
- `CONTRIBUTING.md`
- `SECURITY.md`
- `.github/workflows/ci.yml`
- `THIRD_PARTY_NOTICES.md`

## Recommended Public Release Flow

1. Finish the tree cleanup until the release scan passes.
2. Replace unproven fixtures with synthetic/minimal test fixtures, or move them behind fetch scripts with explicit license notices.
3. Keep `poha-*` crate aliases and runtime IDs consistent.
4. Keep telemetry default-off and document any distribution profile that enables analytics or crash reporting.
5. Build from a clean checkout.
6. Run format, Rust checks, CLI tests, transcription tests, and the macOS app build.
7. Create a fresh public repository or orphan public branch from the final tree.
8. Run secret scanners against that final public tree and history.
9. Make the repository public only after the scans pass.
10. Tag the first release after signing/notarization expectations are decided.

## Recommendation

Do not flip `Iamadig/poha` from private to public in place.

Use one of these instead:

- Best: create a new clean public repo from a tar/archive of the final tree, with one initial commit.
- Acceptable: create an orphan public branch from the final tree, scan that branch history, and publish only that branch.

The one-commit public repo is easier to reason about and avoids accidentally exposing old private history.

## Next Actions

1. Create a clean public-history repository from the current tree only.
2. Remove or replace unproven bundled fixtures and model binaries, or complete `THIRD_PARTY_NOTICES.md` with source and license evidence for each one.
3. Re-run the namespace scan before publishing.
4. Re-run the verification commands below on the final public tree and final public history.
5. Publish only after the final scans pass.

## Current Green Checks

- Current-tree legacy content scan: pass.
- Current tracked filename scan: pass.
- Sensitive filename scan: pass.
- Lock/package legacy scan: pass.
- `gitleaks` current-tree scan: pass.
- `trufflehog` current-tree scan: pass.
- Throwaway one-commit public-history scan: pass.
- `pnpm exec dprint check`: pass.
- `git diff --check`: pass.
- `pnpm cargo:check`: pass.
- `pnpm test:cli`: pass.
- `pnpm test:transcription`: pass.
- `pnpm poha:build`: pass.

## Verification Commands

```sh
pnpm exec dprint check
pnpm cargo:check
pnpm test:cli
pnpm test:transcription
pnpm poha:build
rg -n -i "$OLD_PROJECT_TERMS" --glob '!Cargo.lock' --glob '!pnpm-lock.yaml' --glob '!target/**' --glob '!node_modules/**'
git ls-files | rg -i "\\.(env|pem|key|p12|p8|mobileprovision|cer|crt)$|secret|token|credential|private"
gitleaks dir <public-tree> --redact --max-target-megabytes 20
trufflehog filesystem <public-tree> --force-skip-binaries --force-skip-archives --results=verified,unknown --fail
git log --all --name-only --pretty=format: | rg -i "$OLD_PROJECT_TERMS"
```
