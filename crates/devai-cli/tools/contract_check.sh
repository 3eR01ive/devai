#!/bin/sh
# ux/json-stability probe: CLI-контракт.
# Использование: contract_check.sh <snapshot-dir>
# 1) --help подкоманд не упали; 2) --json везде парсится;
# 3) help-текст не разошёлся со снапшотом (режим --update создаёт/обновляет снапшот).
set -e
BIN=${DEVAI_BIN:-./target/debug/devai}
SNAP=${1:?usage: contract_check.sh <snapshot-dir> [--update]}
UPDATE=0
[ "${2:-}" = "--update" ] && UPDATE=1
mkdir -p "$SNAP"
rc=0

# базовые smoke: команды должны запускаться
for c in "task list" "task --help" "project list" "role list" "contract get --help"; do
  if ! $BIN $c >/dev/null 2>&1; then echo "FAIL: '$BIN $c' не запустилась"; rc=1; fi
done

# json парсится
for c in "task list" "project list"; do
  out=$($BIN $c --json 2>/dev/null)
  if ! printf '%s' "$out" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    echo "FAIL: '$BIN $c --json' — не валидный JSON"; rc=1
  fi
done

# help-снапшоты: дрейф = контракт-брейк (сознательный дрейб подтверждать через --update)
for c in task project role contract agent exec artifact question; do
  f="$SNAP/help_$c.txt"
  $BIN $c --help > /tmp/cc_cur.$$ 2>&1
  if [ "$UPDATE" = 1 ]; then cp /tmp/cc_cur.$$ "$f"; continue; fi
  if [ ! -f "$f" ]; then echo "FAIL: нет снапшота $f (создай: contract_check.sh $SNAP --update)"; rc=1;
  elif ! cmp -s /tmp/cc_cur.$$ "$f"; then echo "FAIL: help '$BIN $c' разошёлся со снапшотом"; diff -u "$f" /tmp/cc_cur.$$ | head -20; rc=1; fi
  rm -f /tmp/cc_cur.$$
done
exit $rc
