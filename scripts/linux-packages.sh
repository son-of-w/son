#!/usr/bin/env bash
# Make a .deb and an .rpm package from a built Linux binary of son.
#
#   scripts/linux-packages.sh <target> <binary> <output folder>
#
# <target> is x86_64-unknown-linux-musl or aarch64-unknown-linux-musl. The
# binary is linked statically, so the packages need nothing else. They hold:
#   /usr/bin/son
#   completion scripts for bash, zsh and fish
#   /usr/share/doc/son/README.md, CHANGELOG.md and the license
# They do not run `son init`, and installing, upgrading or removing them
# does not touch any agent's files or the user's memory.
#
# The files are named son-v<version>-<arch>.deb and son-v<version>-<arch>.rpm,
# so the release script uploads them and lists them in SHA256SUMS.
#
# Needs dpkg-deb for the .deb and rpmbuild for the .rpm (on Ubuntu:
# `sudo apt-get install -y rpm`). A tool that is missing is an error,
# unless SON_PACKAGES="deb" or "rpm" asks for one kind only.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

target="${1:?usage: $0 <target> <binary> <output folder>}"
binary="${2:?usage: $0 <target> <binary> <output folder>}"
out="${3:?usage: $0 <target> <binary> <output folder>}"
kinds="${SON_PACKAGES:-deb rpm}"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
tag="v$version"
summary="One memory for every AI agent"
description="son keeps decisions, patterns, facts and todos in one local SQLite file
that every AI agent on the machine reads and writes. Run 'son init' after
you install it, and again after an upgrade."
homepage="https://son-of-w.com"
maintainer="Xon Designs LLC <contact@xondesigns.com>"

case "$target" in
  x86_64-unknown-linux-musl) deb_arch="amd64"; rpm_arch="x86_64" ;;
  aarch64-unknown-linux-musl) deb_arch="arm64"; rpm_arch="aarch64" ;;
  *) echo "error: no Linux package for the target $target" >&2; exit 1 ;;
esac
[ -x "$binary" ] || { echo "error: $binary is not a binary" >&2; exit 1; }
binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The files of the package, as they are on the user's disk.
tree="$work/tree"
install -D -m 755 "$binary" "$tree/usr/bin/son"
install -D -m 644 README.md "$tree/usr/share/doc/son/README.md"
install -D -m 644 CHANGELOG.md "$tree/usr/share/doc/son/CHANGELOG.md"
install -D -m 644 LICENSE "$tree/usr/share/doc/son/copyright"
# The completion scripts come from the binary itself. It runs here only when
# it was built for this machine's processor.
if "$binary" --version >/dev/null 2>&1; then
  mkdir -p "$tree/usr/share/bash-completion/completions" \
    "$tree/usr/share/zsh/vendor-completions" \
    "$tree/usr/share/fish/vendor_completions.d"
  "$binary" completions bash > "$tree/usr/share/bash-completion/completions/son"
  "$binary" completions zsh > "$tree/usr/share/zsh/vendor-completions/_son"
  "$binary" completions fish > "$tree/usr/share/fish/vendor_completions.d/son.fish"
  chmod 644 "$tree/usr/share/bash-completion/completions/son" \
    "$tree/usr/share/zsh/vendor-completions/_son" \
    "$tree/usr/share/fish/vendor_completions.d/son.fish"
else
  echo "note: $binary does not run on this machine; the packages hold no completion scripts" >&2
fi

for kind in $kinds; do
  case "$kind" in
    deb)
      command -v dpkg-deb >/dev/null || { echo "error: dpkg-deb is needed for the .deb" >&2; exit 1; }
      deb="$work/deb"
      cp -R "$tree" "$deb"
      mkdir -p "$deb/DEBIAN"
      size="$(du -sk "$tree" | cut -f1)"
      {
        echo "Package: son"
        echo "Version: $version"
        echo "Architecture: $deb_arch"
        echo "Maintainer: $maintainer"
        echo "Installed-Size: $size"
        echo "Section: utils"
        echo "Priority: optional"
        echo "Homepage: $homepage"
        echo "Description: $summary"
        printf '%s\n' "$description" | sed 's/^/ /'
      } > "$deb/DEBIAN/control"
      dpkg-deb --build --root-owner-group "$deb" "$out/son-$tag-$deb_arch.deb" >/dev/null
      echo "made $out/son-$tag-$deb_arch.deb"
      ;;
    rpm)
      command -v rpmbuild >/dev/null || { echo "error: rpmbuild is needed for the .rpm" >&2; exit 1; }
      top="$work/rpm"
      mkdir -p "$top/SPECS" "$top/RPMS" "$top/BUILD"
      {
        echo "Name: son"
        echo "Version: $version"
        echo "Release: 1"
        echo "Summary: $summary"
        echo "License: MIT"
        echo "URL: $homepage"
        echo "BuildArch: $rpm_arch"
        # The binary is already built, stripped and static.
        echo "AutoReqProv: no"
        echo "%global debug_package %{nil}"
        echo "%global __strip /bin/true"
        echo "%global __os_install_post %{nil}"
        echo "%description"
        printf '%s\n' "$description"
        echo "%install"
        echo "mkdir -p %{buildroot}"
        echo "cp -a $tree/. %{buildroot}/"
        echo "%files"
        (cd "$tree" && find . -type f | sed 's|^\.||' | sort)
      } > "$top/SPECS/son.spec"
      rpmbuild -bb --quiet --target "$rpm_arch" --define "_topdir $top" "$top/SPECS/son.spec"
      found="$(find "$top/RPMS" -name '*.rpm' | head -1)"
      [ -n "$found" ] || { echo "error: rpmbuild made no package" >&2; exit 1; }
      cp "$found" "$out/son-$tag-$rpm_arch.rpm"
      echo "made $out/son-$tag-$rpm_arch.rpm"
      ;;
    *) echo "error: unknown package kind '$kind' (use deb or rpm)" >&2; exit 1 ;;
  esac
done
