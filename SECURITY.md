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

## Local Recording Control

`poha-cli recording start` and `poha-cli recording stop` are stateful operations that control microphone and system-audio capture in the running app; `recording status` is read-only. Control uses a versioned, size-bounded Unix-socket protocol in Poha's private application-support directory, with restrictive directory/file modes, a random bearer token, and a same-user peer check on macOS.

This is a same-user trust boundary, not isolation from other software running as the logged-in account. A process that can act as the user and read Poha's control directory may be able to start or stop capture. Do not copy, publish, weaken the permissions of, or expose the socket, metadata, or token files. Treat unexpected control activity or permission changes as a security issue.

The standalone app does not configure remote crash reporting. Analytics code is present, but network analytics are inactive unless a build explicitly compiles in a valid `POSTHOG_API_KEY`. Treat any build with analytics or crash reporting enabled as a separate distribution profile that needs its own disclosure before release.

Optional Codex bulk enrichment invokes the local `codex` CLI with access to the recordings directory so it can summarize transcripts. Reports about that path should say whether Codex was installed, which account/backend was configured, and which recordings directory was used.
