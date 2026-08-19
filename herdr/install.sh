#!/usr/bin/env bash
set -euo pipefail

# Install the herdr-mru-cycle binary from a matching GitHub Release.
# This script is invoked by Herdr when the plugin is installed with
# `herdr plugin install`. It can also be run manually for testing.

plugin_root="$(cd "$(dirname "$0")/.." && pwd)"
bin_dir="${HERDR_PLUGIN_BIN_DIR:-$plugin_root/bin}"
mkdir -p "$bin_dir"

version="${HERDR_PLUGIN_VERSION:-0.1.0}"
tag="v${version}"

# Try to derive the repository from git if possible, otherwise fall back.
repo="AlexanderGrooff/herdr-pane-switcher"
if [ -d "$plugin_root/.git" ]; then
    derived_repo="$(git -C "$plugin_root" remote get-url origin 2>/dev/null | sed -E 's#^(https?://github\.com/|git@github\.com:|gh:|)##; s/\.git$//' || true)"
    if [ -n "$derived_repo" ]; then
        repo="$derived_repo"
    fi
fi

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin)
        target_os="apple-darwin"
        ;;
    Linux)
        target_os="unknown-linux-musl"
        ;;
    *)
        echo "Unsupported OS: $os" >&2
        exit 1
        ;;
esac

case "$arch" in
    x86_64)
        target="x86_64-${target_os}"
        ;;
    arm64 | aarch64)
        target="aarch64-${target_os}"
        ;;
    *)
        echo "Unsupported architecture: $arch" >&2
        exit 1
        ;;
esac

asset="herdr-mru-cycle-${target}.tar.gz"
url="https://github.com/${repo}/releases/download/${tag}/${asset}"
checksum_url="${url}.sha256"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading ${asset} from ${repo} ${tag}..."
curl -fsSL "$url" -o "$tmp/${asset}"
curl -fsSL "$checksum_url" -o "$tmp/${asset}.sha256"

expected=$(awk '{print $1}' "$tmp/${asset}.sha256")
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/${asset}" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$tmp/${asset}" | awk '{print $1}')
fi

if [ "$expected" != "$actual" ]; then
    echo "Checksum mismatch for ${asset}" >&2
    exit 1
fi

echo "Extracting binary..."
tar xzf "$tmp/${asset}" -C "$tmp"
mv "$tmp/herdr-mru-cycle" "$bin_dir/herdr-mru-cycle"
chmod +x "$bin_dir/herdr-mru-cycle"

echo "Installed $bin_dir/herdr-mru-cycle"
