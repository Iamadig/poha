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

The standalone app does not configure remote crash reporting. Analytics code is present, but network analytics are inactive unless a build explicitly compiles in a valid `POSTHOG_API_KEY`. Treat any build with analytics or crash reporting enabled as a separate distribution profile that needs its own disclosure before release.

Optional Codex bulk enrichment invokes the local `codex` CLI with access to the recordings directory so it can summarize transcripts. Reports about that path should say whether Codex was installed, which account/backend was configured, and which recordings directory was used.
