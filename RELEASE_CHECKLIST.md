# Release Checklist

This checklist is for maintainers preparing Poha source drops or GitHub Releases.

## Source Preview

- Work from the clean public repository history.
- Run the CI gate from a fresh checkout.
- Run secret scans against the final tree and final git history.
- Keep `README.md`, `SECURITY.md`, `CONTRIBUTING.md`, and `THIRD_PARTY_NOTICES.md` current.
- Re-run stale-name scans before changing repository visibility.
- Keep telemetry default-off unless the build profile has a matching disclosure.

## Binary Release

- Decide whether the build is ad-hoc, signed only, or signed and notarized.
- Document any network analytics, cloud transcription, model downloads, or AI enrichment features enabled in the binary.
- Resolve bundled asset attribution and redistribution decisions in `THIRD_PARTY_NOTICES.md`.
- Build from a clean checkout.
- Run the packaged-app live test on an unlocked macOS desktop session.
- Create a GitHub Release only after the artifact, docs, and security notes match the distribution profile.

## Verification Commands

```sh
pnpm exec dprint check
pnpm cargo:check
pnpm test:cli
pnpm test:lib
pnpm test:transcription
pnpm poha:build
git ls-files | rg -i "\\.(env|pem|key|p12|p8|mobileprovision|cer|crt)$|secret|token|credential|private"
gitleaks detect --source . --no-git --redact --verbose
trufflehog filesystem --no-update --only-verified .
```
