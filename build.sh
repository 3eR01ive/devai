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

# 1) Фронт: зависимости и сборка ассетов ДО cargo — Tauri встраивает dist в бинарь
# на этапе компиляции, поэтому устаревший node_modules или пустой dist даёт либо
# нерезолвленный импорт в dev-режиме, либо бинарь без ассетов (07.11:
# @tauri-apps/plugin-dialog не был установлен после мержа → оверлей вместо окна).
FRONT="crates/devai-ui/frontend"
if [ -d "$FRONT" ]; then
    need_install=0
    [ -d "$FRONT/node_modules/vite" ] || need_install=1
    if [ "$need_install" = 0 ] && [ "$FRONT/package.json" -nt "$FRONT/node_modules" ]; then
        need_install=1
    fi
    if [ "$need_install" = 1 ]; then
        echo "build.sh: ставлю зависимости фронта (npm install)"
        ( cd "$FRONT" && npm install --no-audit --no-fund )
    fi
    echo "build.sh: собираю фронт (vite build)"
    ( cd "$FRONT" && node node_modules/vite/bin/vite.js build )
fi

cargo build --workspace $CARGO_FLAGS

mkdir -p bin
for b in devai devai-ui; do
    if [ -f "target/$PROFILE/$b" ]; then
        # Атомарная подмена: cp поверх РАБОТАЮЩЕГО бинаря даёт ETXTBSY
        # («Text file busy») и валит весь скрипт (set -eu). Копируем рядом
        # и переименовываем — работающий процесс держит старый inode.
        tmp="bin/.$b.new.$$"
        if cp "target/$PROFILE/$b" "$tmp" && mv -f "$tmp" "bin/$b"; then
            echo "bin/$b <- target/$PROFILE/$b"
        else
            rm -f "$tmp"
            echo "build.sh: не удалось обновить bin/$b (занят?) — продолжаю" >&2
        fi
    fi
done
echo "built ($PROFILE). launch: ./bin/devai-ui  (with vite on 127.0.0.1:1420 for debug)"
