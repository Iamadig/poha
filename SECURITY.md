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

The person starting a recording is responsible for participant notice and consent and for complying with applicable laws, contracts, and organizational policies. Poha cannot determine whether capture is lawful or authorized.

## Meeting Detection And Calendar Access

Meeting detection defaults to Off. Ask Before Recording prompts before capture. Ask With Calendar Context may enrich that prompt with a fresh, sanitized calendar occurrence, but provider and time cannot prove that the active process is that occurrence. Native, power-only, unscheduled, and browser evidence therefore remain prompt-only; meeting detection never starts capture autonomously.

Native detection uses stable application identifiers and coarse CoreAudio/IOKit activity; it does not inspect window titles, screen contents, URLs, or meeting content. Calendar matching is separately opt-in and requests EventKit full access through the visible macOS permission sheet. Poha queries only a bounded near-current window and reduces matching events to provider, start/end times, and a one-way occurrence hash. Titles, attendees, notes, organizers, locations, raw URLs, meeting IDs, passwords, and URL tokens are not retained or passed into Poha's occurrence model.

The standalone app does not configure remote crash reporting. Analytics code is present, but network analytics are inactive unless a build explicitly compiles in a valid `POSTHOG_API_KEY`. Treat any build with analytics or crash reporting enabled as a separate distribution profile that needs its own disclosure before release.

Optional Codex bulk enrichment invokes the local `codex` CLI with access to the recordings directory so it can summarize transcripts. Reports about that path should say whether Codex was installed, which account/backend was configured, and which recordings directory was used.
