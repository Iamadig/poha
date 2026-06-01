# Contributing

Thanks for helping improve Poha.

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
pnpm test:transcription
```

Use focused commits with Conventional Commit prefixes such as `fix:`, `feat:`, `docs:`, and `test:`.
