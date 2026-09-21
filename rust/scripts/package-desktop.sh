#!/usr/bin/env bash
# Example packaging hook for desktop-release.yml. Replace this script in an
# application template, but preserve its <target-label> <output-dir> contract.
set -euo pipefail

LABEL="${1:?target label is required}"
OUTPUT_DIR="${2:?output directory is required}"
TAG="${DESKTOP_RELEASE_TAG:-dev}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CRATE_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
REPOSITORY_ROOT="$(cd -- "$CRATE_ROOT/.." && pwd)"

if [[ "$OUTPUT_DIR" != /* ]]; then
  OUTPUT_DIR="$REPOSITORY_ROOT/$OUTPUT_DIR"
fi

mkdir -p "$OUTPUT_DIR"
cd "$CRATE_ROOT"
cargo build --release

binary=target/release/package-registry-manager
extension=""
if [[ "$LABEL" == windows-* ]]; then
  binary+=".exe"
  extension=".exe"
fi
[ -s "$binary" ] || { echo "Expected binary was not built: $binary" >&2; exit 1; }

asset="package-registry-manager-${LABEL}-${TAG}${extension}"
cp "$binary" "$OUTPUT_DIR/$asset"
echo "Packaged $OUTPUT_DIR/$asset"
