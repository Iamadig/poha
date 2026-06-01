# Contributing

Poha is a local-first macOS app, so changes should preserve local recording reliability and user control over meeting data.

## Setup

```sh
pnpm install
pnpm cargo:check
```

## Local Development

```sh
pnpm poha:dev
```

## Checks

Run the relevant checks before opening a pull request:

```sh
pnpm exec dprint fmt
pnpm cargo:check
pnpm test:cli
pnpm test:lib
pnpm test:transcription
```

Use focused commits with Conventional Commit prefixes such as `fix:`, `feat:`, `docs:`, and `test:`. Update `README.md`, `SECURITY.md`, or `THIRD_PARTY_NOTICES.md` when behavior, privacy posture, bundled assets, or release assumptions change.
