#!/usr/bin/env bash
# Build the son binaries for the version in Cargo.toml and publish them as a
# GitHub release. The Release workflow runs this script, one part in each
# job; you can also run it on your own machine when GitHub Actions is not
# working.
#
#   scripts/release.sh            check, package, then publish
#   scripts/release.sh package    build the binaries for this machine into dist/
#   scripts/release.sh publish    publish what is in dist/ as a GitHub release
#
# Parts of publish, which the workflow runs one at a time:
#   draft     open a draft release for the tag, if there is none
#   upload    add the archives in dist/ to the draft
#   finish    add SHA256SUMS for every archive and make the release public
#
# Which binaries are built:
#   by default, the ones this machine can build (on a Mac, both Mac binaries)
#   SON_TARGETS="<target> <target>"   exactly these
#   SON_TARGETS=all                   every target of a full release
# A target that needs another system is built with cargo-zigbuild when that
# is installed, and is otherwise left out with a note.
#
# The release is for the tag v<version>. The binaries are always built from
# the tagged commit: if the tag already exists and is not the commit you are
# on, they are built from a temporary copy of the tagged commit.
#
# Every part can be run again. Nothing is public until `finish`.

set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

# Every target of a full release.
ALL_TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-msvc
)
DIST="$ROOT/dist"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
TAG="v$VERSION"
# True on a GitHub runner, where the tag is already checked out and pushed.
IN_CI="${GITHUB_ACTIONS:-false}"
SYSTEM="$(uname -s)"

say() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Run a command up to four times: GitHub's services sometimes fail briefly.
retry() {
  local attempt=1
  until "$@"; do
    if [ "$attempt" -ge 4 ]; then
      echo "gave up after $attempt attempts: $*" >&2
      return 1
    fi
    echo "attempt $attempt failed; trying again in $((attempt * 15))s: $*" >&2
    sleep $((attempt * 15))
    attempt=$((attempt + 1))
  done
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

tag_exists() { git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; }

require_clean_tree() {
  [ -z "$(git status --porcelain)" ] ||
    die "there are uncommitted changes; commit or stash them first"
}

# The targets to build, one on each line.
wanted_targets() {
  case "${SON_TARGETS:-}" in
    all) printf '%s\n' "${ALL_TARGETS[@]}" ;;
    "")
      case "$SYSTEM" in
        Darwin) printf '%s\n' aarch64-apple-darwin x86_64-apple-darwin ;;
        Linux) echo "$(uname -m)-unknown-linux-musl" ;;
        MINGW* | MSYS* | CYGWIN*) echo x86_64-pc-windows-msvc ;;
        *) die "do not know which targets to build on $SYSTEM; set SON_TARGETS" ;;
      esac
      ;;
    *) printf '%s\n' $SON_TARGETS ;;
  esac
}

# How to build a target here: "cargo", "zigbuild", or "" when it cannot be.
builder_for() {
  local target="$1"
  case "$SYSTEM:$target" in
    Darwin:*-apple-darwin) echo cargo ;;
    Linux:"$(uname -m)"-unknown-linux-musl) echo cargo ;;
    MINGW*:*-windows-msvc | MSYS*:*-windows-msvc | CYGWIN*:*-windows-msvc) echo cargo ;;
    *:*-windows-msvc) echo "" ;; # needs the Microsoft toolchain
    *) if cargo zigbuild --version >/dev/null 2>&1; then echo zigbuild; else echo ""; fi ;;
  esac
}

check() {
  say "Checking (format, clippy, tests)"
  make check
}

package() {
  [ -n "$VERSION" ] || die "cannot read the version from Cargo.toml"
  local src="$ROOT" copy=""

  if tag_exists && [ "$(git rev-parse "$TAG^{commit}")" != "$(git rev-parse HEAD)" ]; then
    # The tag is an earlier commit: build from a copy of exactly that commit.
    copy="$(mktemp -d)"
    say "$TAG is not the current commit; building from a copy of $TAG"
    git worktree add --quiet --detach "$copy/src" "$TAG"
    trap 'git worktree remove --force "'"$copy"'/src" 2>/dev/null; rm -rf "'"$copy"'"' EXIT
    src="$copy/src"
    grep -q "^version = \"$VERSION\"$" "$src/Cargo.toml" ||
      die "$TAG holds a different version than $VERSION"
  elif [ "$IN_CI" != "true" ]; then
    require_clean_tree
  fi

  rm -rf "$DIST"
  mkdir -p "$DIST"
  local target builder binary skipped=()
  while read -r target; do
    builder="$(builder_for "$target")"
    if [ -z "$builder" ]; then
      skipped+=("$target")
      continue
    fi
    say "Building $target"
    rustup target add "$target" >/dev/null
    # SQLite is C code. For a musl build on Linux, compile it with musl-gcc,
    # which the musl-tools package installs under that one name on every
    # processor type.
    if [ "$builder" = cargo ] && [[ "$target" == *-linux-musl ]] && command -v musl-gcc >/dev/null; then
      export "CC_${target//-/_}=musl-gcc"
    fi
    if [ "$builder" = zigbuild ]; then
      (cd "$src" && cargo zigbuild --release --locked --target "$target" --target-dir "$ROOT/target")
    else
      (cd "$src" && cargo build --release --locked --target "$target" --target-dir "$ROOT/target")
    fi
    case "$target" in
      *-windows-*)
        binary="son.exe"
        if command -v zip >/dev/null; then
          (cd "$ROOT/target/$target/release" && zip -q "$DIST/son-$TAG-$target.zip" "$binary")
        else
          (cd "$ROOT/target/$target/release" && 7z a -bso0 "$DIST/son-$TAG-$target.zip" "$binary")
        fi
        ;;
      *)
        tar -czf "$DIST/son-$TAG-$target.tar.gz" -C "$ROOT/target/$target/release" son
        ;;
    esac
  done < <(wanted_targets)

  if [ "${#skipped[@]}" -gt 0 ]; then
    [ "$IN_CI" != "true" ] || die "cannot build here: ${skipped[*]}"
    say "Left out, because this machine cannot build them: ${skipped[*]}"
  fi
  ls "$DIST"/son-"$TAG"-* >/dev/null 2>&1 || die "nothing was built"
  (cd "$DIST" && sha256 son-"$TAG"-* > SHA256SUMS)
  say "Packaged in dist/:"
  cat "$DIST/SHA256SUMS"
}

# Make sure the tag exists here and on GitHub.
push_tag() {
  if ! tag_exists; then
    require_clean_tree
    git fetch --quiet origin
    git merge-base --is-ancestor HEAD origin/main ||
      die "this commit is not on GitHub's main yet; push it first"
    say "Tagging this commit as $TAG"
    git tag "$TAG"
  fi
  if [ -z "$(git ls-remote --tags origin "refs/tags/$TAG")" ]; then
    say "Pushing the tag $TAG"
    retry git push origin "$TAG"
  fi
}

# Pushing a tag also starts the Release workflow. Stop any run of it that has
# not finished, so that it and this script do not both upload files.
stop_workflow_runs() {
  local ids
  ids="$(gh run list --workflow Release --branch "$TAG" --json databaseId,status \
    --jq '.[] | select(.status != "completed") | .databaseId' 2>/dev/null || true)"
  for id in $ids; do
    say "Stopping the Release workflow run $id for $TAG"
    gh run cancel "$id" >/dev/null 2>&1 || true
  done
}

need_gh() { command -v gh >/dev/null || die "the GitHub CLI (gh) is needed to publish"; }

# A draft, so nothing is public until every file is uploaded.
ensure_release() {
  gh release view "$TAG" >/dev/null 2>&1 ||
    gh release create "$TAG" --draft --title "son $TAG" --generate-notes --verify-tag
}

draft() {
  need_gh
  say "Opening a draft release for $TAG"
  retry ensure_release
}

upload() {
  need_gh
  ls "$DIST"/son-"$TAG"-* >/dev/null 2>&1 ||
    die "dist/ has no archives for $TAG; run '$0 package' first"
  say "Uploading to $TAG:"
  ls "$DIST"/son-"$TAG"-* | sed 's|.*/|  |'
  retry gh release upload "$TAG" "$DIST"/son-"$TAG"-* --clobber
}

# Checksums are made from the files GitHub holds, so that SHA256SUMS always
# matches what a user downloads, whichever machines built the archives.
finish() {
  need_gh
  local held
  held="$(mktemp -d)"
  retry gh release download "$TAG" --pattern "son-$TAG-*" --dir "$held" --clobber
  ls "$held"/son-"$TAG"-* >/dev/null 2>&1 || die "the release $TAG holds no archives"

  if [ "${SON_REQUIRE_ALL:-false}" = "true" ]; then
    local target
    for target in "${ALL_TARGETS[@]}"; do
      ls "$held"/son-"$TAG"-"$target".* >/dev/null 2>&1 ||
        die "the release $TAG has no archive for $target"
    done
  fi

  (cd "$held" && sha256 son-"$TAG"-* > SHA256SUMS)
  say "The release holds:"
  cat "$held/SHA256SUMS"
  retry gh release upload "$TAG" "$held/SHA256SUMS" --clobber
  retry gh release edit "$TAG" --draft=false >/dev/null
  rm -rf "$held"
  say "Published: $(gh release view "$TAG" --json url --jq .url)"
}

publish() {
  need_gh
  if [ "$IN_CI" != "true" ]; then
    push_tag
    # Give GitHub a moment to register the run the tag started.
    sleep 5
    stop_workflow_runs
  fi
  draft
  upload
  finish
}

case "${1:-all}" in
  package) package ;;
  draft) draft ;;
  upload) upload ;;
  finish) finish ;;
  publish) publish ;;
  all)
    check
    package
    publish
    ;;
  *)
    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
