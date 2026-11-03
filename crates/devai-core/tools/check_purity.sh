#!/bin/sh
# arch/layer-boundaries probe: чистота слоёв devai.
# Эмитит violations (file:line hit) в stdout; exit 0 = violations нет, exit 1 = есть.
# Границы: core не знает про SQLite/файлы/процессы/сеть; runtime не дублирует
# правила переходов ядра (это rubric-часть hybrid-проверки, не grep).
set -- devai-core
rc=0
for pat in 'rusqlite' 'std::fs' 'std::process' 'reqwest\|hyper\|ureq\|TcpStream'; do
  hits=$(grep -RIn "$pat" "crates/$1/src" 2>/dev/null)
  if [ -n "$hits" ]; then echo "$hits"; rc=1; fi
done
# store не импортирует cli/runtime (направление зависимостей)
for dep in devai-cli devai-runtime; do
  hits=$(grep -RIn "use $dep\|extern crate $dep" crates/devai-store/src crates/devai-core/src 2>/dev/null)
  if [ -n "$hits" ]; then echo "$hits"; rc=1; fi
done
exit $rc
