#!/usr/bin/env bash
# Installs spillway from a GitHub release (or from the extracted package this
# script ships in): binary, systemd units and an example config if none exists.
#
#   curl -fsSL https://github.com/WilianZilv/spillway/releases/latest/download/install.sh | sudo bash
#   curl -fsSL .../install.sh | sudo bash -s -- --version v0.2.0 --role node
#
# Options:
#   --version vX.Y.Z   release to install (default: latest)
#   --role master|node which service to set up (default: master)
#   --enable           enable and start the service right away
set -euo pipefail

REPO="${SPILLWAY_REPO:-WilianZilv/spillway}"
VERSION=latest
ROLE=master
ENABLE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --role) ROLE="$2"; shift 2 ;;
    --enable) ENABLE=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
[ "$ROLE" = master ] || [ "$ROLE" = node ] || { echo "--role must be master or node" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run as root (sudo)" >&2; exit 1; }

case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=arm64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
PKG="spillway-linux-$ARCH"

# Use the files next to this script when it runs from an extracted package.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || echo .)"
if [ -x "$HERE/spillway" ] && [ -d "$HERE/deploy" ]; then
  SRC="$HERE"
else
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  if [ "$VERSION" = latest ]; then BASE="https://github.com/$REPO/releases/latest/download"; else BASE="https://github.com/$REPO/releases/download/$VERSION"; fi
  echo "downloading $PKG ($VERSION)"
  curl -fsSL -o "$TMP/$PKG.tar.gz" "$BASE/$PKG.tar.gz"
  curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS"
  (cd "$TMP" && grep " $PKG.tar.gz\$" SHA256SUMS | sha256sum -c -)
  tar -C "$TMP" -xzf "$TMP/$PKG.tar.gz"
  SRC="$TMP/$PKG"
fi

install -m 755 "$SRC/spillway" /usr/local/bin/spillway
mkdir -p /etc/spillway
if [ "$ROLE" = master ]; then
  [ -e /etc/spillway/spillway.toml ] || install -m 640 "$SRC/deploy/spillway.example.toml" /etc/spillway/spillway.toml
  install -m 644 "$SRC/deploy/spillway-master.service" /etc/systemd/system/
else
  [ -e /etc/spillway/node.env ] || install -m 640 "$SRC/deploy/node.env.example" /etc/spillway/node.env
  install -m 644 "$SRC/deploy/spillway-node.service" /etc/systemd/system/
fi
command -v systemctl >/dev/null && systemctl daemon-reload || true
command -v fusermount3 >/dev/null || echo "warning: fusermount3 not found; install the fuse3 package" >&2

echo "installed $(/usr/local/bin/spillway --version)"
if [ "$ENABLE" = 1 ]; then
  systemctl enable --now "spillway-$ROLE"
elif [ "$ROLE" = master ]; then
  echo "next: edit /etc/spillway/spillway.toml, run 'spillway check', then 'systemctl enable --now spillway-master'"
else
  echo "next: edit /etc/spillway/node.env, then 'systemctl enable --now spillway-node'"
fi
