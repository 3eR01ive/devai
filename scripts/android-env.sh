#!/usr/bin/env bash
# Окружение для сборки под Android — подключается из ./build.sh --android.
# Всё лежит в домашнем каталоге пользователя и нужно только для этих сборок:
#   ~/Android/jdk-21       JDK для Gradle (системная Java слишком новая для AGP)
#   ~/Android/Sdk          Android SDK + NDK (cmdline-tools/latest/bin/sdkmanager)
#   ~/Android/rustup-bin   отдельный rustup с установленным aarch64-linux-android,
#                          чтобы не менять системный Rust
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export JAVA_HOME="${ANDROID_JAVA_HOME:-$HOME/Android/jdk-21}"
if [ -z "${NDK_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
  NDK_HOME="$ANDROID_HOME/ndk/$(ls "$ANDROID_HOME/ndk" | sort -V | tail -1)"
  export NDK_HOME
fi
export PATH="$HOME/Android/rustup-bin:$JAVA_HOME/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"

# Собираем только arm64: шим заставляет `tauri android` не докачивать остальные таргеты.
_shim="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/android-shim"
export PATH="$_shim:$PATH"

# Если в тулчейне rustup нет рабочего cargo — ведём его rustc (с Android std)
# системным cargo.
_tc="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin"
if [ ! -x "$_tc/cargo" ] && [ -x "$_tc/rustc" ] && [ -x /usr/bin/cargo ]; then
  export RUSTC="$_tc/rustc"
  mkdir -p "$_shim" && ln -sf /usr/bin/cargo "$_shim/cargo"
else
  rm -f "$_shim/cargo"
fi

missing=()
[ -x "$JAVA_HOME/bin/java" ] || missing+=("JDK 21 ($JAVA_HOME)")
[ -d "$ANDROID_HOME/platforms" ] || missing+=("Android SDK ($ANDROID_HOME)")
if [ -n "${NDK_HOME:-}" ] && [ -d "$NDK_HOME" ]; then :; else missing+=("Android NDK"); fi
if [ -x "$HOME/Android/rustup-bin/rustup" ] && "$HOME/Android/rustup-bin/rustup" target list --installed 2>/dev/null | grep -q aarch64-linux-android; then :; else missing+=("rustup + target aarch64-linux-android"); fi
if [ ${#missing[@]} -gt 0 ]; then
  echo "Не хватает для сборки под Android: ${missing[*]}" >&2
  return 1 2>/dev/null || exit 1
fi
