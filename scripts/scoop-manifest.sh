#!/usr/bin/env bash
# Print a Scoop manifest for a published release of son.
#
#   scripts/scoop-manifest.sh [version]     default: the version in Cargo.toml
#
# The bucket is the repository son-of-w/scoop-bucket; the output goes there
# as bucket/son.json, and users install with:
#   scoop bucket add son-of-w https://github.com/son-of-w/scoop-bucket
#   scoop install son
# The checksum comes from the release's SHA256SUMS. Scoop does not run
# `son init`.
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)}"
tag="v$version"
repo="son-of-w/son"
archive="son-$tag-x86_64-pc-windows-msvc.zip"
sums="$(gh release download "$tag" --repo "$repo" --pattern SHA256SUMS --output -)" ||
  { echo "error: no published release $tag with SHA256SUMS" >&2; exit 1; }
hash="$(printf '%s\n' "$sums" | awk -v f="$archive" '$2 == f || $2 == "*" f {print $1}')"
[ -n "$hash" ] || { echo "error: $tag has no archive $archive" >&2; exit 1; }
releases="https://github.com/$repo/releases/download"
cat <<MANIFEST
{
    "version": "$version",
    "description": "One memory for every AI agent: one binary, one SQLite file",
    "homepage": "https://son-of-w.com",
    "license": "MIT",
    "architecture": {
        "64bit": {
            "url": "$releases/$tag/$archive",
            "hash": "$hash"
        }
    },
    "bin": "son.exe",
    "notes": [
        "Run 'son init' to set son up for your AI agents.",
        "After an upgrade, run 'son init' again: it upgrades the database and the agent files.",
        "Scoop does not change any agent's files and does not touch your memory.",
        "To take son out of your agents' files, run 'son uninstall' before 'scoop uninstall son'."
    ],
    "checkver": {
        "github": "https://github.com/$repo"
    },
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "$releases/v\$version/son-v\$version-x86_64-pc-windows-msvc.zip"
            }
        },
        "hash": {
            "url": "\$baseurl/SHA256SUMS"
        }
    }
}
MANIFEST
