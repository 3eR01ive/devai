#!/bin/sh
# devai build helper (manual operator tooling, ratified by operator word 06.11):
#   ./build.sh            -> debug build
#   ./build.sh --release  -> release build
# Resulting binaries are copied into ./bin/ (git-ignored except .keep), so
# `bin/devai` / `bin/devai-ui` are the canonical launch paths and a stale
# ./target/... binary can never masquerade as fresh after a schema bump.
set -eu

PROFILE="debug"
CARGO_FLAGS=""
for arg in "$@"; do
    case "$arg" in
        --release) PROFILE="release"; CARGO_FLAGS="--release" ;;
        -h|--help)
            echo "usage: ./build.sh [--release]  # binaries land in ./bin/"; exit 0 ;;
        *) echo "build.sh: unknown argument: $arg" >&2; exit 2 ;;
    esac
done

cargo build --workspace $CARGO_FLAGS

mkdir -p bin
for b in devai devai-ui; do
    if [ -f "target/$PROFILE/$b" ]; then
        cp "target/$PROFILE/$b" "bin/$b"
        echo "bin/$b <- target/$PROFILE/$b"
    fi
done
echo "built ($PROFILE). launch: ./bin/devai-ui  (with vite on 127.0.0.1:1420 for debug)"
