#!/usr/bin/env bash
# Подпись Android-сборки devai-ui — шаг, который делает ./build.sh --android.
#
# Зачем: без ключа Gradle собирает app-universal-release-unsigned.apk, а такой
# файл нельзя поставить на телефон (Android отказывает: «APK is not signed»).
# Ключ локальный, лежит в crates/devai-ui/gen/android (каталог gen/ в .gitignore).
#
# Android ставит обновление поверх установленного приложения только если подпись
# та же. Ключ не пересоздавать: потеря ключа = пользователю придётся удалять
# приложение перед установкой.
set -euo pipefail

GEN="crates/devai-ui/gen/android"
PROPS="$GEN/keystore.properties"
KS_NAME="devai-ui-release.jks"
ALIAS="devai-ui"
PASS="devai-ui"

[ -d "$GEN" ] || { echo "android-signing: нет $GEN — сначала cargo tauri android init" >&2; exit 1; }

if [ ! -f "$GEN/$KS_NAME" ]; then
    echo "android-signing: создаю ключ $KS_NAME (alias $ALIAS)"
    keytool -genkeypair \
        -keystore "$GEN/$KS_NAME" \
        -alias "$ALIAS" \
        -keyalg RSA -keysize 4096 -validity 10000 \
        -storepass "$PASS" -keypass "$PASS" \
        -dname "CN=devai-ui, O=devai, C=RU" >/dev/null
fi

cat > "$PROPS" <<EOF
storeFile=$KS_NAME
keyAlias=$ALIAS
password=$PASS
EOF

python3 - "$GEN/app/build.gradle.kts" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1])
s = p.read_text(encoding="utf-8")
if "keystoreProperties" in s:
    print("android-signing: build.gradle.kts уже знает про ключ")
    sys.exit(0)

anchor = "android {\n"
loader = (
    "val keystoreProperties = Properties().apply {\n"
    "    val propFile = rootProject.file(\"keystore.properties\")\n"
    "    if (propFile.exists()) {\n"
    "        propFile.inputStream().use { load(it) }\n"
    "    }\n"
    "}\n\n"
)
s = s.replace(anchor, loader + anchor, 1)

signing = (
    "    signingConfigs {\n"
    "        create(\"release\") {\n"
    "            if (keystoreProperties.containsKey(\"storeFile\")) {\n"
    "                storeFile = rootProject.file(keystoreProperties.getProperty(\"storeFile\"))\n"
    "                storePassword = keystoreProperties.getProperty(\"password\")\n"
    "                keyAlias = keystoreProperties.getProperty(\"keyAlias\")\n"
    "                keyPassword = keystoreProperties.getProperty(\"password\")\n"
    "            }\n"
    "        }\n"
    "    }\n"
)
s = s.replace("    buildTypes {\n", signing + "    buildTypes {\n", 1)

# release: подписывать, если ключ есть
old = "        getByName(\"release\") {\n            isMinifyEnabled = true\n"
new = ("        getByName(\"release\") {\n"
       "            if (keystoreProperties.containsKey(\"storeFile\")) "
       "signingConfig = signingConfigs.getByName(\"release\")\n"
       "            isMinifyEnabled = true\n")
if old in s:
    s = s.replace(old, new, 1)
else:
    print("android-signing: не нашёл блок release в build.gradle.kts", file=sys.stderr)
    sys.exit(1)

p.write_text(s, encoding="utf-8")
print("android-signing: build.gradle.kts подписывает release локальным ключом")
PY
