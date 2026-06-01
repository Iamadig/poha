#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

case "${1:-dev}" in
  dev)
    pnpm -F poha tauri:dev
    ;;
  check)
    pnpm cargo:check
    ;;
  build)
    pnpm poha:build
    ;;
  live-test)
    ./script/run_live_test.sh
    ;;
  *)
    echo "usage: $0 [dev|check|build|live-test]" >&2
    exit 2
    ;;
esac
