#!/bin/sh
# devai-ui dev launcher (operator tooling).
#
# Debug-бинарь ходит на http://localhost:1420 — окно без vite показывает
# «Operation was cancelled»/белое полотно. Скрипт поднимает фронт-сервер,
# дожидается ответа и запускает UI; сервер гасится вместе с окном.
#
#   ./run-ui.sh                 # DEVAI_HOME=$HOME/.devai (по умолчанию)
#   ./run-ui.sh --home /path    # другой дом
#   ./run-ui.sh --release       # запустить bin/devai-ui из release-сборки
#                               # (standalone по-прежнему сломан upstream wry#1725:
#                               #  webkit>=2.46 custom-scheme; debug-путь — рабочий)
set -eu

ROOT=$(cd "$(dirname "$0")" && pwd)
FRONT="$ROOT/crates/devai-ui/frontend"
PORT=1420
HOME_DIR=${DEVAI_HOME:-$HOME/.devai}
PROFILE=debug

while [ $# -gt 0 ]; do
    case "$1" in
        --home) [ $# -ge 2 ] || { echo "run-ui.sh: --home needs a path" >&2; exit 2; }
                HOME_DIR=$2; shift 2 ;;
        --release) PROFILE=release; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "run-ui.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done

BIN="$ROOT/bin/devai-ui"

# 1) Бинарь: если нет — собрать (и положить в ./bin, как build.sh).
if [ ! -x "$BIN" ]; then
    echo "run-ui.sh: bin/devai-ui отсутствует — собираю ($PROFILE) через ./build.sh"
    if [ "$PROFILE" = release ]; then "$ROOT/build.sh" --release; else "$ROOT/build.sh"; fi
fi

# 2) Зависимости фронта (node_modules в git не живёт).
if [ ! -d "$FRONT/node_modules/vite" ]; then
    echo "run-ui.sh: ставлю зависимости фронта (npm install)"
    (cd "$FRONT" && npm install --no-audit --no-fund)
fi

# 3) Дом должен существовать: UI читает его стор и реестр домов.
if [ ! -d "$HOME_DIR" ]; then
    echo "run-ui.sh: домашний каталог $HOME_DIR не найден — создаю через devai init"
    mkdir -p "$HOME_DIR"
    "$ROOT/bin/devai" init --home "$HOME_DIR" >/dev/null
fi

started_vite=0
vite_pid=
cleanup() {
    if [ "$started_vite" = 1 ] && [ -n "$vite_pid" ] && kill -0 "$vite_pid" 2>/dev/null; then
        kill "$vite_pid" 2>/dev/null || true
        wait "$vite_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

# 4) Фронт-сервер: если 1420 уже отвечает — используем его (не плодим второй
#    на strictPort), иначе поднимаем свой на 127.0.0.1 (бинарь ждёт localhost).
if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null; then
    echo "run-ui.sh: vite уже слушает $PORT — использую существующий"
else
    echo "run-ui.sh: поднимаю vite на 127.0.0.1:$PORT"
    ( cd "$FRONT" && exec node node_modules/vite/bin/vite.js --host 127.0.0.1 --port "$PORT" --strictPort ) \
        >"${TMPDIR:-/tmp}/devai-ui-vite.log" 2>&1 &
    vite_pid=$!
    started_vite=1
    i=0
    while [ $i -lt 100 ]; do
        if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null; then break; fi
        if ! kill -0 "$vite_pid" 2>/dev/null; then
            echo "run-ui.sh: vite упал — лог: ${TMPDIR:-/tmp}/devai-ui-vite.log" >&2
            tail -20 "${TMPDIR:-/tmp}/devai-ui-vite.log" >&2 || true
            exit 1
        fi
        i=$((i + 1)); sleep 0.2
    done
    if [ $i -ge 100 ]; then
        echo "run-ui.sh: vite не ответил за 20 с — лог: ${TMPDIR:-/tmp}/devai-ui-vite.log" >&2
        exit 1
    fi
fi

# 5) UI. WEBKIT_DISABLE_COMPOSITING_MODE=1 — известный обход на webkit>=2.46
#    (без него окно мигает/пустое на части конфигураций).
echo "run-ui.sh: запускаю UI (дом $HOME_DIR, $PROFILE)"
# НЕ exec: оболочка должна дожить до закрытия окна, чтобы trap погасил vite
# (иначе фронт-сервер остаётся сиротой до следующего запуска).
env WEBKIT_DISABLE_COMPOSITING_MODE=1 DEVAI_HOME="$HOME_DIR" "$BIN"
