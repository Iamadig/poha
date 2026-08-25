# Security Policy

## Reporting A Vulnerability

Please do not open public issues for security reports.

Email the repository owner or use GitHub private vulnerability reporting once it is enabled for the public repository. Include:

- Affected version or commit
- Reproduction steps
- Expected impact
- Any logs or files needed to understand the issue, with private meeting content removed

## Data Handling

Poha records meetings locally. Security reports should avoid attaching real meeting audio, transcripts, credentials, tokens, or personal data unless explicitly requested through a private channel.

Poha preserves microphone and system-audio stems as source evidence when stem preservation is enabled. Record Only is the default for new installs. Existing settings that lack a `recordingMode` retain the earlier Record-and-Transcribe behavior; explicit settings remain authoritative, and legacy session manifests without that field are treated as Record-and-Transcribe during recovery.

Crash recovery acquires a per-session operating-system lease before reading or changing the manifest and fails closed while capture or finalization is active. Verified capture-scratch duplicates are moved to macOS Trash only after durable copies compare byte-for-byte.

The person starting a recording is responsible for participant notice and consent and for complying with applicable laws, contracts, and organizational policies. Poha cannot determine whether capture is lawful or authorized.

The standalone app does not configure remote crash reporting. Analytics code is present, but network analytics are inactive unless a build explicitly compiles in a valid `POSTHOG_API_KEY`. Treat any build with analytics or crash reporting enabled as a separate distribution profile that needs its own disclosure before release.

Optional Codex bulk enrichment invokes the local `codex` CLI with access to the recordings directory so it can summarize transcripts. Reports about that path should say whether Codex was installed, which account/backend was configured, and which recordings directory was used.
