#!/bin/sh
# Подготовка сгенерированного Android-проекта к открытию приглашения внутри
# приложения (task 01a1bf78, REQ-QRAPP-02).
#
# crates/devai-ui/gen/android — артефакт `cargo tauri android init` и в git не
# попадает, поэтому единственное, чего Tauri не умеет объявлять из
# tauri.conf.json (разрешение cleartext), приводится здесь одной идемпотентной
# правкой. Сам intent-filter схемы `devai://pair` НЕ правится руками: его пишет
# build-script плагина deep-link из `plugins.deep-link.mobile` в tauri.conf.json,
# а здесь только проверяется, что конфиг на месте.
#
# Вызывается из ./build.sh --android после init, до сборки APK.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP="$ROOT/crates/devai-ui/gen/android/app"

[ -d "$APP" ] || {
    echo "android-pair: нет $APP — сначала (cd crates/devai-ui && cargo tauri android init)" >&2
    exit 1
}

# 1) Очистка трафика. Интерфейс дома отдаётся по http на адресе в домашней сети
#    (TLS на LAN-адресе взять неоткуда), а release-сборка Tauri по умолчанию
#    ставит usesCleartextTraffic=false — с ним окно приложения не откроет дом,
#    и кнопка «открыть в приложении» снова приведёт в никуда.
GRADLE="$APP/build.gradle.kts"
python3 - "$GRADLE" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
before = 'manifestPlaceholders["usesCleartextTraffic"] = "false"'
after = 'manifestPlaceholders["usesCleartextTraffic"] = "true"'
# Только ПЕРВОЕ (общее, release) значение: у debug оно уже true.
path.write_text(text.replace(before, after, 1), encoding="utf-8")
PY

grep -q 'manifestPlaceholders\["usesCleartextTraffic"\] = "true"' "$GRADLE" || {
    echo "android-pair: cleartext traffic не включился в $GRADLE" >&2
    exit 1
}
echo "android-pair: cleartext traffic разрешён (release)"

# 2) Схема приглашения объявлена в конфиге, из которого плагин строит
#    intent-filter. Пустая секция = APK не заявляет `devai://` и браузер
#    останется обработчиком — ровно та жалоба, с которой задача началась.
CONF="$ROOT/crates/devai-ui/tauri.conf.json"
grep -q '"scheme": \["devai"\]' "$CONF" || {
    echo "android-pair: в tauri.conf.json нет схемы devai (plugins.deep-link)" >&2
    exit 1
}
grep -q '"host": "pair"' "$CONF" || {
    echo "android-pair: в tauri.conf.json нет хоста pair (devai://pair)" >&2
    exit 1
}
echo "android-pair: схема devai://pair объявлена"
