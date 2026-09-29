#!/bin/sh
# Installs the `annox` binary from a GitHub release, and optionally the agent skill.
#
#   curl -fsSL https://raw.githubusercontent.com/gaardhus/annox/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/gaardhus/annox/main/install.sh | sh -s -- --skill
#
# Options (or environment variables):
#   --version vX.Y.Z   ANNOX_VERSION      release to install (default: the latest)
#   --dir DIR          ANNOX_INSTALL_DIR  where to put `annox` (default: ~/.local/bin)
#   --skill            ANNOX_SKILL=1      also install the skill for Claude Code
#   --skill-dir DIR    ANNOX_SKILL_DIR    where skills go (default: ~/.claude/skills)

set -eu

repo="gaardhus/annox"
version="${ANNOX_VERSION:-}"
bin_dir="${ANNOX_INSTALL_DIR:-$HOME/.local/bin}"
skill="${ANNOX_SKILL:-0}"
skill_dir="${ANNOX_SKILL_DIR:-$HOME/.claude/skills}"

say() { printf 'annox: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

usage() {
  cat << 'EOF'
Usage: install.sh [--version vX.Y.Z] [--dir DIR] [--skill] [--skill-dir DIR]

  --version vX.Y.Z  release to install (default: the latest; or ANNOX_VERSION)
  --dir DIR         where to put `annox` (default: ~/.local/bin; or ANNOX_INSTALL_DIR)
  --skill           also install the skill for Claude Code (or ANNOX_SKILL=1)
  --skill-dir DIR   where skills go (default: ~/.claude/skills; or ANNOX_SKILL_DIR)
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --dir) bin_dir="${2:?--dir needs a value}"; shift 2 ;;
    --skill) skill=1; shift ;;
    --skill-dir) skill_dir="${2:?--skill-dir needs a value}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

command -v curl > /dev/null || die "curl is required"
command -v tar > /dev/null || die "tar is required"

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) die "unsupported OS: $(uname -s). On Windows, download the .zip from https://github.com/$repo/releases" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
target="$arch-$os"
case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | aarch64-apple-darwin) ;;
  *) die "no prebuilt binary for $target; build from source with: cargo install --git https://github.com/$repo annox-lsp" ;;
esac

if [ -z "$version" ]; then
  # The latest release page redirects to its tag.
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") ||
    die "could not find the latest release"
  version="${url##*/}"
fi
case "$version" in
  v*) ;;
  *) version="v$version" ;;
esac

name="annox-$version-$target"
base="https://github.com/$repo/releases/download/$version"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $name"
curl -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz" || die "could not download $base/$name.tar.gz"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "could not download $base/SHA256SUMS"

(
  cd "$tmp"
  grep " $name.tar.gz\$" SHA256SUMS > expected || die "$name.tar.gz is not in SHA256SUMS"
  if command -v sha256sum > /dev/null; then
    sha256sum -c expected > /dev/null
  elif command -v shasum > /dev/null; then
    shasum -a 256 -c expected > /dev/null
  else
    die "sha256sum or shasum is required to verify the download"
  fi
) || die "checksum mismatch for $name.tar.gz"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$bin_dir"
cp "$tmp/$name/annox" "$bin_dir/annox.tmp"
chmod 755 "$bin_dir/annox.tmp"
mv -f "$bin_dir/annox.tmp" "$bin_dir/annox"
say "installed $bin_dir/annox ($version)"

if [ "$skill" = 1 ]; then
  [ -d "$tmp/$name/skills/annox" ] || die "$version has no skill in its release archive"
  mkdir -p "$skill_dir"
  rm -rf "${skill_dir:?}/annox"
  cp -R "$tmp/$name/skills/annox" "$skill_dir/annox"
  say "installed the skill in $skill_dir/annox"
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) say "$bin_dir is not on your PATH; add it to use \`annox\`" ;;
esac
