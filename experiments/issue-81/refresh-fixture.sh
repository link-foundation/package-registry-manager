#!/usr/bin/env bash
# Refreshes tests/fixtures/links-notation from a links-notation checkout
# (issue #81: ten-language repository at 0.30.0, SHA 5ce8287c).
# Usage: experiments/issue-81/refresh-fixture.sh /path/to/links-notation
set -euo pipefail
src=$1
dest=$(dirname "$0")/../../tests/fixtures/links-notation
expected=$(cat "$dest/expected-packages.json")
find "$dest" -mindepth 1 -delete
(cd "$src" && find . -path ./.git -prune -o -path ./node_modules -prune -o -type f \( \
  -name CMakeLists.txt -o -name conanfile.py -o -name vcpkg.json -o -name package.json \
  -o -name Cargo.toml -o -name pyproject.toml -o -name setup.py -o -name go.mod \
  -o -name pom.xml -o -name 'build.gradle*' -o -name 'settings.gradle*' \
  -o -name gradle.properties -o -name '*.versions.toml' -o -name composer.json \
  -o -name jsr.json -o -name deno.json -o -name '*.csproj' -o -name Package.swift \
  -o -name VERSION -o -name global.json -o -name mvnw -o -name gradlew \
  -o -name gradle-wrapper.properties -o -name LICENSE \) -print) |
  grep -v '/node_modules/' | grep -v '^./docs/' |
  while read -r file; do
    mkdir -p "$dest/$(dirname "$file")"
    cp "$src/$file" "$dest/$file"
  done
mkdir -p "$dest/.github/workflows"
cp "$src"/.github/workflows/*.yml "$dest/.github/workflows/"
printf '%s\n' "$expected" > "$dest/expected-packages.json"
