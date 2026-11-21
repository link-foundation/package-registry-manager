#!/usr/bin/env bash
set -euo pipefail

report=$(mktemp)
trap 'rm -f "$report"' EXIT

if RUST_LOG=warn cargo audit --file rust/Cargo.lock --deny warnings --json > "$report"; then
  echo 'Cargo.lock passed the strict RustSec audit.'
  exit 0
else
  audit_status=$?
fi

cat "$report"
if [[ "$audit_status" != 1 ]]; then
  exit "$audit_status"
fi

# Keep the full audit visible. Temporarily allow these exact baseline findings
# on PRs while upstream maintainers publish crates with the fixes already merged.
sqlite='["RUSTSEC-2026-0459", "libsqlite3-sys", "0.38.2"]'
chrono='["RUSTSEC-2026-0543", "chrono", "0.4.45"]'
if ! summary=$(jq -e -c -s --argjson sqlite "$sqlite" --argjson chrono "$chrono" '
  def finding: [.advisory.id, .package.name, .package.version];
  select(length == 1) | .[0] |
  select((.vulnerabilities.list | type) == "array" and (.warnings | type) == "object") |
  select(.vulnerabilities.count == (.vulnerabilities.list | length)) |
  [.vulnerabilities.list[] | ["vulnerability", finding]] as $vulnerabilities |
  [.warnings | to_entries[] | .key as $kind | .value[] | [$kind, finding]] as $warnings |
  ($vulnerabilities + $warnings) as $findings |
  ["vulnerability", $sqlite] as $sqlite_finding |
  ["unmaintained", $chrono] as $chrono_finding |
  {
    sqlite: any($findings[]; . == $sqlite_finding),
    chrono: any($findings[]; . == $chrono_finding),
    baseline_only: (($findings | length) > 0 and ($findings - [$sqlite_finding, $chrono_finding]) == [])
  }
' "$report"); then
  exit "$audit_status"
fi

if [[ "${GITHUB_EVENT_NAME:-}" == pull_request ]] && jq -e '.baseline_only' <<< "$summary" > /dev/null; then
  echo '::warning title=Temporary Cargo audit exception::Only the existing RUSTSEC-2026-0459 and RUSTSEC-2026-0543 findings are allowed on pull requests while patched upstream crates await publication. Main and release audits remain strict.'
  exit 0
fi

if jq -e '.sqlite' <<< "$summary" > /dev/null; then
  echo '::error title=Upgrade Browser Commander for patched SQLite::Publish a browser-commander crate containing https://github.com/link-foundation/browser-commander/commit/065ac59aaeeb91b1460924b48dbbbade7d006264 to crates.io. Raise its minimum version in rust/Cargo.toml, run cargo update --manifest-path rust/Cargo.toml -p browser-commander, and verify the lockfile uses libsqlite3-sys 0.39.0 or later.'
fi
if jq -e '.chrono' <<< "$summary" > /dev/null; then
  echo '::error title=Upgrade Command Stream and remove Chrono::Publish a command-stream crate containing https://github.com/link-foundation/command-stream/pull/240 to crates.io. Raise its minimum version and migrate this crate away from its direct chrono dependency in rust/Cargo.toml and Rust code, then run cargo update --manifest-path rust/Cargo.toml -p command-stream and verify the lockfile contains no chrono.'
fi
if jq -e '.sqlite or .chrono' <<< "$summary" > /dev/null; then
  echo 'Commit the updated manifest and lockfile and rerun the failing workflow on the updated commit.'
fi
exit "$audit_status"
