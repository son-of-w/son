#!/usr/bin/env bash
# Build the Mac binaries for the version in Cargo.toml and publish them as a
# GitHub release. The Release workflow runs this script; so can you, on your
# own Mac, when GitHub Actions is not working.
#
#   scripts/release.sh            check, package, then publish
#   scripts/release.sh package    build both Mac binaries into dist/
#   scripts/release.sh publish    publish what is in dist/ as a GitHub release
#
# The release is for the tag v<version>. The binaries are always built from
# the tagged commit: if the tag already exists and is not the commit you are
# on, they are built from a temporary copy of the tagged commit.
#
# Every part can be run again. Nothing is public until every file is uploaded.

set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
DIST="$ROOT/dist"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
TAG="v$VERSION"
# True on a GitHub runner, where the tag is already checked out and pushed.
IN_CI="${GITHUB_ACTIONS:-false}"

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

tag_exists() { git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; }

require_clean_tree() {
  [ -z "$(git status --porcelain)" ] ||
    die "there are uncommitted changes; commit or stash them first"
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
  else
    require_clean_tree
  fi

  rustup target add "${TARGETS[@]}" >/dev/null
  rm -rf "$DIST"
  mkdir -p "$DIST"
  for target in "${TARGETS[@]}"; do
    say "Building $target"
    (cd "$src" && cargo build --release --locked --target "$target" --target-dir "$ROOT/target")
    tar -czf "$DIST/son-$TAG-$target.tar.gz" -C "$ROOT/target/$target/release" son
  done
  (cd "$DIST" && shasum -a 256 ./*.tar.gz | sed 's|  \./|  |' > SHA256SUMS)
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

# A draft, so nothing is public until every file is uploaded.
ensure_release() {
  gh release view "$TAG" >/dev/null 2>&1 ||
    gh release create "$TAG" --draft --title "son $TAG" --generate-notes --verify-tag
}

publish() {
  [ -f "$DIST/SHA256SUMS" ] || die "dist/ is empty; run '$0 package' first"
  ls "$DIST"/son-"$TAG"-*.tar.gz >/dev/null 2>&1 ||
    die "dist/ holds another version than $TAG; run '$0 package' again"
  command -v gh >/dev/null || die "the GitHub CLI (gh) is needed to publish"

  if [ "$IN_CI" != "true" ]; then
    push_tag
    # Give GitHub a moment to register the run the tag started.
    sleep 5
    stop_workflow_runs
  fi

  say "Publishing $TAG"
  retry ensure_release
  retry gh release upload "$TAG" "$DIST"/*.tar.gz "$DIST/SHA256SUMS" --clobber
  retry gh release edit "$TAG" --draft=false >/dev/null
  say "Published: $(gh release view "$TAG" --json url --jq .url)"
}

case "${1:-all}" in
  package) package ;;
  publish) publish ;;
  all)
    check
    package
    publish
    ;;
  *)
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
