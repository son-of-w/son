#!/usr/bin/env bash
# Print a Homebrew formula for a published release of son.
#
#   scripts/homebrew-formula.sh [version]     default: the version in Cargo.toml
#
# The formula installs the binary that the release holds for the Mac's
# processor (Apple silicon or Intel) and for Linux, with the checksums from
# the release's SHA256SUMS file. It does not run `son init`.
# Put the output in a tap as Formula/son.rb (see todos/12.md).
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)}"
tag="v$version"
repo="Raxvis/son-of-w"
sums="$(gh release download "$tag" --repo "$repo" --pattern SHA256SUMS --output -)" ||
  { echo "error: no published release $tag with SHA256SUMS" >&2; exit 1; }
sum() {
  local found
  found="$(printf '%s\n' "$sums" | awk -v f="son-$tag-$1.tar.gz" '$2 == f || $2 == "*" f {print $1}')"
  [ -n "$found" ] || { echo "error: $tag has no archive for $1" >&2; exit 1; }
  echo "$found"
}
base="https://github.com/$repo/releases/download/$tag/son-$tag"
cat <<FORMULA
class Son < Formula
  desc "Local shared memory for AI agents: one binary, one SQLite file"
  homepage "https://github.com/$repo"
  version "$version"
  license :cannot_represent

  on_macos do
    on_arm do
      url "$base-aarch64-apple-darwin.tar.gz"
      sha256 "$(sum aarch64-apple-darwin)"
    end
    on_intel do
      url "$base-x86_64-apple-darwin.tar.gz"
      sha256 "$(sum x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$base-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sum aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$base-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sum x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "son"
  end

  def caveats
    <<~EOS
      Run \`son init\` to set son up for your AI agents.
      Installing son does not change any agent's files.
    EOS
  end

  test do
    assert_match "son #{version}", shell_output("#{bin}/son --version")
    ENV["SON_DB"] = testpath/"son.db"
    system bin/"son", "remember", "note", "brew test"
    assert_match "brew test", shell_output("#{bin}/son recall brew")
  end
end
FORMULA
