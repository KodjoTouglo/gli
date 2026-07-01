#!/bin/sh
# gli installer: downloads the right prebuilt binary from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/KodjoTouglo/gli/develop/install.sh | sh
#
# Override version with VPSGUARD_VERSION, install dir with VPSGUARD_BIN_DIR.
set -eu

REPO="KodjoTouglo/gli"
BIN="gli"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)
    case "$arch" in
      x86_64 | amd64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64) target="aarch64-apple-darwin" ;;
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "unsupported OS: $os (download the Windows .zip from the Releases page)" >&2
    exit 1
    ;;
esac

tag="${VPSGUARD_VERSION:-}"
if [ -z "$tag" ]; then
  tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name":' | head -1 | cut -d'"' -f4)
fi
[ -n "$tag" ] || { echo "could not resolve the latest version" >&2; exit 1; }

asset="${BIN}-${tag}-${target}.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset ..."
curl -fsSL "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"

dir="${VPSGUARD_BIN_DIR:-/usr/local/bin}"
[ -w "$dir" ] 2>/dev/null || dir="$HOME/.local/bin"
mkdir -p "$dir"
cp "$tmp/$BIN" "$dir/$BIN"
chmod 0755 "$dir/$BIN"

echo "Installed $BIN $tag to $dir/$BIN"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH to run gli." ;;
esac
