#!/bin/sh
# Install son on macOS or Linux.
#
#   curl -fsSL https://son-of-w.com/install.sh | sh
#
# It downloads the binary for this system from the GitHub release, checks
# it against the release's SHA256SUMS, and puts it in ~/.local/bin.
#
#   SON_VERSION=1.0.0      install this version (default: the newest release)
#   SON_INSTALL_DIR=<dir>  put the binary here (default: ~/.local/bin)
#
# It does not run `son init`, does not change any agent's files, and does
# not touch your memory (~/.son-of-w). Run it again to upgrade.
# To remove son: `son uninstall`, then delete the binary.
set -eu

repo="son-of-w/son"
dir="${SON_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

# Fetch a URL to a file.
fetch() {
  if have curl; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    die "curl or wget is needed"
  fi
}

# Where a URL leads after its redirects.
final_url() {
  if have curl; then
    curl -fsSLI -o /dev/null -w '%{url_effective}' "$1"
  else
    wget -q -S --spider "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -1 | tr -d '\r'
  fi
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "sha256sum or shasum is needed to check the download"
  fi
}

case "$(uname -s)" in
  Darwin) system="apple-darwin" ;;
  Linux) system="unknown-linux-musl" ;;
  *) die "this installer is for macOS and Linux. On Windows, use install.ps1 or Scoop" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) machine="x86_64" ;;
  arm64 | aarch64) machine="aarch64" ;;
  *) die "there is no son binary for the processor type $(uname -m)" ;;
esac
target="$machine-$system"
have tar || die "tar is needed"

version="${SON_VERSION:-}"
if [ -z "$version" ]; then
  latest="$(final_url "https://github.com/$repo/releases/latest")" ||
    die "cannot reach GitHub to find the newest release"
  version="${latest##*/tag/v}"
  case "$version" in
    "" | *[!0-9.]*) die "cannot read the newest version from $latest; set SON_VERSION" ;;
  esac
fi
version="${version#v}"
tag="v$version"
archive="son-$tag-$target.tar.gz"
base="https://github.com/$repo/releases/download/$tag"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

say "Downloading son $version for $target"
fetch "$base/$archive" "$work/$archive" || die "cannot download $base/$archive"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || die "cannot download $base/SHA256SUMS"

want="$(awk -v f="$archive" '$2 == f || $2 == "*" f {print $1}' "$work/SHA256SUMS")"
[ -n "$want" ] || die "SHA256SUMS of $tag does not list $archive"
got="$(sha256_of "$work/$archive")"
[ "$want" = "$got" ] || die "the checksum of $archive is $got, but the release says $want. Nothing was installed"

tar -xzf "$work/$archive" -C "$work"
[ -f "$work/son" ] || die "$archive does not hold the son binary"
"$work/son" --version >/dev/null 2>&1 || die "the downloaded binary does not run on this system"

mkdir -p "$dir" || die "cannot create $dir; set SON_INSTALL_DIR to a folder you can write to"
# Put it in place in one step, so that a running son is never half written.
cp "$work/son" "$dir/.son.new.$$"
chmod 755 "$dir/.son.new.$$"
mv -f "$dir/.son.new.$$" "$dir/son"

say "Installed: $dir/son ($("$dir/son" --version))"
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    say ""
    say "$dir is not on your PATH. Add this line to the startup file of your shell:"
    say "  export PATH=\"$dir:\$PATH\""
    ;;
esac
say ""
say "Next: run 'son init' to set son up for your AI agents."
say "After an upgrade, run 'son init' again: it upgrades the database and the agent files."
say "Your memory in ~/.son-of-w was not touched."
