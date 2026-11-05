# Генератор known-bad образца (REQ-QA-03). Вечерний оригинал 05.11.2026:
# webkit2gtk 2.50.6 не диспатчил top-level навигацию кастомной схемы
# tauri://localhost и рисовал error page «Operation was cancelled» (som-дамп
# сохранён в истории hermes-сессии). На webkit 2.52.6 реальный release-бинарь
# этот путь больше не воспроизводит (upstream, см. wry#1725), поэтому скрипт
# воспроизводит ТОТ ЖЕ механизм на живом webkit этого машины: обработчик
# схемы tauri:// завершает main-resource навигацию ошибкой G_IO_ERROR_CANCELLED
# («Operation was cancelled») — webkit рендерит собственный error page с этим
# текстом. Окно живёт ~60 с; снимок пары — ui_content.py snap --pid.
import os

import gi
gi.require_version('Gtk', '3.0')
gi.require_version('WebKit2', '4.1')
from gi.repository import Gtk, WebKit2, GLib

CANCELLED = 19  # G_IO_ERROR_CANCELLED


def on_scheme(request):
    error = GLib.Error(domain=GLib.quark_from_string('g-io-error-quark'),
                       code=CANCELLED, message='Operation was cancelled')
    request.finish_error(error)


ctx = WebKit2.WebContext.get_default()
ctx.register_uri_scheme('tauri', on_scheme)

win = Gtk.Window(title='devai-ui-standalone-repro')
win.set_default_size(1100, 750)
wv = WebKit2.WebView.new_with_context(ctx)
wv.load_uri('tauri://localhost')
win.add(wv)
win.show_all()
loop = GLib.MainLoop()
GLib.timeout_add_seconds(60, loop.quit)
print('PID', os.getpid(), flush=True)
loop.run()
