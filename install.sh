#!/bin/sh
# devai installer — no sudo required.
#
# One-liner:
#   curl -fsSL https://raw.githubusercontent.com/3eR01ive/devai/main/install.sh | sh
#
# What it does: downloads the linux-x86_64 release tarball, verifies its
# sha256, and installs the `devai` binary into $HOME/.local/bin (override
# with DEVAI_INSTALL_DIR). Existing installs are replaced.
#
# Environment:
#   DEVAI_VERSION      release tag to install (default: latest)
#   DEVAI_INSTALL_DIR  target directory (default: $HOME/.local/bin)
#   DEVAI_BASE         base URL of release assets (default: GitHub)
set -eu

REPO="3eR01ive/devai"
OS=$(uname -s)
ARCH=$(uname -m)

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

if ! echo ":$PATH:" | tr '[:space:]' ':' | grep -qs ":$DEST:"; then
    echo
    echo "  NOTE: $DEST is not in your PATH. Add to your shell profile:"
    echo "    export PATH=\"$DEST:\$PATH\""
fi
echo "  Try it: devai --help"
