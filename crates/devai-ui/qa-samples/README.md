# Регрессионные образцы критерия «содержимое окна» (REQ-QA-03)

Пара артефактов на образец = `screenshot.png` + `axtree.txt` (формат
`ui_content.py snap`). Проверяет их `ui_content.py selftest`; пороги —
`../tools/ui_expectations.json`.

## known-bad — «окно живо, содержимое error-page»

Вечер 05.11.2026, webkit2gtk 2.50.6: standalone release-бинарь devai-ui —
top-level навигация `tauri://localhost` молча отменялась (не CORS-enabled
схема; upstream-регрессия webkit ≥2.46, фикс wry#1725 в релиз не влит),
webview рисовал error page «Operation was cancelled». Процесс жив, окна нет
по contents — ровно то, что раньше проходило QA.

- `axtree.txt` — подлинные элементы вечернего accessibility-дампа окна
  «Devai-ui» (som-capture hermes computer_use, 3 элемента, 1 текстовый),
  сконвертированы из `original-som-11111.json` (верbatim-копия источника —
  history hermes-сессии, id сообщения 11111). Хедер файла описывает источник.
- `screenshot.png` — воспроизведение того же отказа на webkit 2.52.6 этой машины:
  `repro_error_page.py` регистрирует схему `tauri://` с ошибкой
  G_IO_ERROR_CANCELLED («Operation was cancelled», тот же механизм отмены
  main-resource навигации) — webkit 2.52 не рендерит текст error-page в окно,
  кадр показывает белую страницу того же класса. Половина пары — фото
  живого окна с отменённой навигацией, а не вечерний оригинал: оригинальный
  снимок окна на диск не сохранялся (som-картинка была эфемерной), вечерний
  AX-дамп — сохранён и лежит здесь.
- Воспроизводимость: `DISPLAY=:0 python3 repro_error_page.py` в другой сессии
  + `ui_content.py snap --pid <PID>`; либо `check` прямо на этом каталоге.

Вердикт харнесса на образце: FAIL (эталонный error-page текст + пустое
дерево 3 < 20 узлов + отсутствующая навигация) — самоконтроль `selftest`.

## known-good — живой прогон

05.11.2026, dev-путь: vite `--host 127.0.0.1 --port 1420` (unit devai-fe4) +
`target/debug/devai-ui` (unit devai-ui-dev4) — тот же прогон, что в
постановке (рабочие devai-fe4/devai-ui-dev4). Снят `ui_content.py snap`
в `evidence/<task>/<exec>/ui/live-dev/` и зафиксирован здесь: 26 узлов
AX-дерева, пять навигационных ссылок, статус-бар, Projects view с данными;
скриншот окна целевого процесса (`import -window`).

Вердикт харнесса: PASS.

Оба образца проверены на webkit 2.52.6 машины; механизм отказа известной
схемы зафиксирован data-половиной known-bad (подлинный вечерний дамп).
