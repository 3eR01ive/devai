#!/bin/sh
# devai installer — no sudo required, everything stays under $HOME.
#
# One-liner:
#   curl -fsSL https://raw.githubusercontent.com/3eR01ive/devai/main/install.sh | sh
#
# What it does:
#   1. downloads the linux-x86_64 release tarball and verifies its sha256;
#   2. installs `devai` (CLI) and `devai-ui` (window) into $HOME/.local/bin;
#   3. materializes the home ($DEVAI_HOME, default ~/.devai) by running
#      `devai init` — config tree and store — unless a home already exists or
#      --no-init is given (an existing home is never touched);
#   4. adds an application-menu entry + icon for the window (unless
#      --no-desktop, or the release carries no window);
#   5. checks the window's system libraries (webkit2gtk, gtk3, rsvg) and names
#      what is missing together with the package hint for this distribution;
#   6. prints the one PATH line you need — or appends it with --add-path.
#
# Flags (pass after `sh -s --` when piping):
#   --no-init      do not create the home
#   --no-desktop   do not add the application-menu entry
#   --add-path     append the PATH line to your shell profile (guarded, once)
#   -h, --help     show this help
#
# Environment:
#   DEVAI_VERSION      release tag to install (default: latest)
#   DEVAI_INSTALL_DIR  target directory (default: $HOME/.local/bin)
#   DEVAI_HOME         home directory to create (default: $HOME/.devai)
#   DEVAI_BASE         base URL of release assets (default: GitHub)
set -eu

REPO="3eR01ive/devai"
OS=$(uname -s)
ARCH=$(uname -m)

WANT_INIT=1
WANT_DESKTOP=1
WANT_PATH=0

while [ $# -gt 0 ]; do
    case "$1" in
        --no-init) WANT_INIT=0 ;;
        --no-desktop) WANT_DESKTOP=0 ;;
        --add-path) WANT_PATH=1 ;;
        -h|--help) sed -n '2,29p' "$0"; exit 0 ;;
        *) echo "devai installer: unknown argument '$1' (try --help)" >&2; exit 2 ;;
    esac
    shift
done

if [ "$OS" != "Linux" ]; then
    echo "devai installer: unsupported OS '$OS' (this script installs Linux x86_64 binaries)" >&2
    exit 1
fi
case "$ARCH" in
    x86_64|amd64) : ;;
    *) echo "devai installer: unsupported architecture '$ARCH' (Linux x86_64 only for now)" >&2; exit 1 ;;
esac

for tool in curl tar sha256sum; do
    command -v "$tool" >/dev/null || { echo "devai installer: need '$tool' in PATH" >&2; exit 1; }
done

BASE=${DEVAI_BASE:-https://github.com/$REPO/releases}
if [ -n "${DEVAI_VERSION:-}" ]; then
    VERSION="$DEVAI_VERSION"
else
    VERSION=$(curl -fsSL "$BASE/latest" | sed -n 's|.*'"$REPO"'/releases/tag/\([^"]*\).*|\1|p' | head -1)
    [ -n "$VERSION" ] || { echo "devai installer: could not resolve the latest release" >&2; exit 1; }
fi

ASSET="devai-$VERSION-linux-x86_64"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "devai installer: fetching $ASSET.tar.gz ($VERSION)"
curl -fsSL -o "$TMP/$ASSET.tar.gz" "$BASE/download/$VERSION/$ASSET.tar.gz"
curl -fsSL -o "$TMP/$ASSET.tar.gz.sha256" "$BASE/download/$VERSION/$ASSET.tar.gz.sha256"

( cd "$TMP" && sha256sum -c "$ASSET.tar.gz.sha256" ) \
    || { echo "devai installer: checksum mismatch, refusing to install" >&2; exit 1; }

tar xzf "$TMP/$ASSET.tar.gz" -C "$TMP"

DEST=${DEVAI_INSTALL_DIR:-$HOME/.local/bin}
mkdir -p "$DEST"
rm -f "$DEST/devai"
install -m 0755 "$TMP/$ASSET/devai" "$DEST/devai"
echo "devai installer: installed $DEST/devai ($VERSION)"

HAVE_UI=0
# Окно лежит рядом в том же архиве; старые релизы его не содержат — тогда
# честно говорим, что окна нет, а не делаем вид, что поставили.
if [ -f "$TMP/$ASSET/devai-ui" ]; then
    rm -f "$DEST/devai-ui"
    install -m 0755 "$TMP/$ASSET/devai-ui" "$DEST/devai-ui"
    HAVE_UI=1
    echo "devai installer: installed $DEST/devai-ui ($VERSION) — окно (Tauri)"
else
    echo "devai installer: этот релиз без окна (в архиве нет devai-ui)"
fi

case ":$PATH:" in
    *":$DEST:"*) : ;;
    *)
        echo
        echo "  NOTE: $DEST is not in your PATH. Add to your shell profile:"
        echo "    export PATH=\"$DEST:\$PATH\""
        if [ "$WANT_PATH" = 1 ]; then
            # Профиль выбираем по текущей оболочке; правило добавляем ОДИН раз
            # (grep-страховка), чтобы повторный запуск не копил строки.
            case "${SHELL:-}" in
                *zsh) PROFILE="$HOME/.zshrc" ;;
                *bash) PROFILE="$HOME/.bashrc" ;;
                *) PROFILE="$HOME/.profile" ;;
            esac
            LINE="export PATH=\"$DEST:\$PATH\""
            [ -f "$PROFILE" ] || : > "$PROFILE"
            if grep -qsF "$LINE" "$PROFILE"; then
                echo "  --add-path: строка уже есть в $PROFILE"
            else
                printf '\n# devai installer\n%s\n' "$LINE" >> "$PROFILE"
                echo "  --add-path: дописал в $PROFILE (перезапустите оболочку или source $PROFILE)"
            fi
        fi
        ;;
esac

# Дом: без него ни CLI, ни окно ничего не знают. Создаём только когда его нет —
# существующий дом не трогаем никогда (это данные человека).
HOME_DIR=${DEVAI_HOME:-$HOME/.devai}
if [ "$WANT_INIT" = 1 ]; then
    if [ -d "$HOME_DIR" ]; then
        echo "devai installer: дом $HOME_DIR уже есть — init не запускаю (данные не трогаем)"
    else
        echo "devai installer: создаю дом $HOME_DIR (devai init)"
        if DEVAI_HOME="$HOME_DIR" "$DEST/devai" init >/dev/null 2>&1; then
            echo "devai installer: дом готов — конфиг, роли и стор на месте"
        else
            echo "devai installer: devai init не прошёл — запустите вручную: DEVAI_HOME=$HOME_DIR devai init" >&2
        fi
    fi
else
    echo "devai installer: --no-init — дом не создавал; потом: devai init"
fi

# Окно в меню: ярлык + иконка в пользовательских данных (ничего в /usr).
if [ "$HAVE_UI" = 1 ] && [ "$WANT_DESKTOP" = 1 ]; then
    APPS="$HOME/.local/share/applications"
    ICONS="$HOME/.local/share/icons/hicolor/512x512/apps"
    mkdir -p "$APPS" "$ICONS"
    ICON_FILE="$ICONS/devai-ui.png"
    if [ -f "$TMP/$ASSET/devai-ui.png" ]; then
        install -m 0644 "$TMP/$ASSET/devai-ui.png" "$ICON_FILE"
        ICON_VALUE="$ICON_FILE"
    else
        ICON_VALUE="applications-development"
    fi
    cat > "$APPS/devai-ui.desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=devai
Comment=Local-first multi-agent development platform
Exec=$DEST/devai-ui
Icon=$ICON_VALUE
Terminal=false
Categories=Development;
StartupWMClass=devai-ui
ENTRY
    chmod 0644 "$APPS/devai-ui.desktop"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" >/dev/null 2>&1 || true
    echo "devai installer: ярлык в меню — $APPS/devai-ui.desktop"
fi

# Библиотеки окна: установщик их не ставит (это система), но обязан НАЗВАТЬ,
# чего не хватает, — иначе окно молча не стартует.
if [ "$HAVE_UI" = 1 ] && command -v ldd >/dev/null 2>&1; then
    MISSING=$(ldd "$DEST/devai-ui" 2>/dev/null | awk '/not found/ {print $1}' | sort -u | tr '\n' ' ')
    if [ -n "$MISSING" ]; then
        echo
        echo "  ВНИМАНИЕ: окну не хватает системных библиотек: $MISSING" >&2
        echo "  Поставьте их пакетным менеджером, например:" >&2
        if command -v pacman >/dev/null 2>&1; then
            echo "    sudo pacman -S webkit2gtk-4.1 gtk3 librsvg" >&2
        elif command -v apt-get >/dev/null 2>&1; then
            echo "    sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0 librsvg2-2" >&2
        elif command -v dnf >/dev/null 2>&1; then
            echo "    sudo dnf install webkit2gtk4.1 gtk3 librsvg2" >&2
        else
            echo "    нужны webkit2gtk 4.1, gtk3 и librsvg (имена пакетов — по вашему дистрибутиву)" >&2
        fi
        echo "  CLI работает и без них: devai --help" >&2
    fi
fi

echo
echo "  Try it: devai --help"
if [ "$HAVE_UI" = 1 ]; then
    echo "  Window: devai-ui (или из меню приложений)"
fi
echo "  Home:   $HOME_DIR"
