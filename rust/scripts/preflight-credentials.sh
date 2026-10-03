#!/usr/bin/env bash
#
# Decide, per registry, whether this run can publish -- before any expensive
# job runs.
#
# Principle 16 of the shared CI/CD best practices ("Prove You Can Publish
# Before You Build", issues #163 and #167), reworked for package-registry-manager
# issue #16: every registry is probed independently and gets its own verdict,
# so one refused registry no longer blocks the others. The verdicts are
# written to $GITHUB_OUTPUT as `crates=`, `npm=` and `docker=`; each
# publishing job gates on its own verdict being `ok`.
#
# Verdicts:
#   ok        -- this run can publish to the registry.
#   bootstrap -- the package does not exist on the registry yet. Trusted
#                publishing (OIDC) can only publish *new versions* of a package
#                that already exists, so the first version has to be published
#                once from a maintainer's machine:
#                  package-registry-manager setup --registry <registry> --execute
#   refused   -- the registry refused the credential (Docker Hub only: it is
#                the one registry this workflow still reaches with a
#                long-lived token).
#   unknown   -- the probe got no verdict (timeout, 429, 5xx). Not a guess
#                that the credential is broken -- but not a pass either.
#   skipped   -- publishing to the registry is not configured.
#
# Registries:
#   crates.io -- published with trusted publishing (rust-lang/crates-io-auth-action).
#                There is no long-lived token left to probe: the short-lived
#                token is minted by an OIDC exchange in the publishing job, the
#                only job that holds `id-token: write`. The probe here is
#                whether the crate exists (GET /api/v1/crates/<name>).
#   npm       -- published with npm trusted publishing (OIDC). Same reasoning:
#                the probe is whether the package exists on the registry.
#   Docker Hub-- still a username + token. A login -- or a registry token
#                endpoint -- proves authentication, not authorisation:
#                auth.docker.io answers an anonymous pull,push request with 200
#                and silently narrows the grant to pull. The only form of the
#                check that is not a guess is an attempted write:
#                POST /v2/<repo>/blobs/uploads/ -> 202 opens an upload session,
#                DELETE cancels it, nothing is stored and no tag moves.
#
# PREFLIGHT_MODE:
#   release -- push to main / manual instant release. A registry whose verdict
#              is not `ok` is annotated as a problem and its publishing job is
#              skipped; the other registries still publish.
#   report  -- pull requests, where a fork legitimately has no publishing
#              secrets. The same probes run and annotate, but never block.
#
# The script exits 0 in both modes: the gate is the per-registry output, not
# the job result. (A crashed script still fails the job, and every publishing
# job additionally requires `needs.release-preflight.result == 'success'`.)
#
# Rules each caller depends on (each is a defect if dropped):
#   1. Report every registry, not the first -- no probe aborts the script.
#   2. Report `unknown`, never a guess -- and only `ok` publishes.
#   3. Probe a long-lived credential with a write, not a login.
#   4. Never print a credential.
#
# No set -e on purpose: rule 1 means one failed probe must not hide the rest.

set -u

# The crates.io probe reads the package manifest. Keep the script callable
# from the repository root now that the Rust package lives in rust/.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [ ! -f Cargo.toml ] && [ -f "$SCRIPT_DIR/../Cargo.toml" ]; then
  cd "$SCRIPT_DIR/.."
fi

MODE="${PREFLIGHT_MODE:-report}"
CRATES_API="${CRATES_API:-https://crates.io}"
NPM_REGISTRY="${NPM_REGISTRY:-https://registry.npmjs.org}"
NPM_PACKAGE_JSON="${NPM_PACKAGE_JSON:-$SCRIPT_DIR/../../js/package.json}"
DOCKER_REGISTRY="${DOCKER_REGISTRY:-https://registry-1.docker.io}"
DOCKER_AUTH="${DOCKER_AUTH:-https://auth.docker.io}"
CURL_TIMEOUT="${PREFLIGHT_CURL_TIMEOUT:-15}"
NEWLINE=$'\n'

# crates.io answers 403 to API calls without a User-Agent.
CURL_USER_AGENT="release-preflight (github.com/link-foundation/package-registry-manager)"

crates_verdict='skipped'
npm_verdict='skipped'
docker_verdict='skipped'
# One line per registry whose verdict is not ok/skipped: "<level>|<message>".
problems=''

pass() {
  printf '  PASS: %s\n' "$*"
}

skip() {
  printf '  SKIP: %s\n' "$*"
}

# problem <verdict> <message>
problem() {
  local verdict="$1" message="$2" label
  label=$(printf '%s' "$verdict" | tr '[:lower:]' '[:upper:]')
  problems="${problems}${problems:+${NEWLINE}}${verdict}|${message}"
  printf '  %s: %s\n' "$label" "$message"
}

# curl that separates the HTTP status from the body without temp files.
# Prints "body\nstatus"; a network failure yields an empty status, which the
# callers treat as unknown.
http() {
  local body
  body=$(curl -sS --max-time "$CURL_TIMEOUT" -o - -w "${NEWLINE}%{http_code}" "$@" 2>/dev/null)
  printf '%s\n%s' "${body%"${NEWLINE}"*}" "${body##*"$NEWLINE"}"
}

http_status() {
  local response
  response=$(http "$@")
  printf '%s' "${response##*"$NEWLINE"}"
}

# The [package].name from Cargo.toml, parsed section-aware the same way
# scripts/rust-paths.rs reads manifests: a `name` under `[dependencies]` or
# any other table must never be mistaken for the package name.
crate_name_from_manifest() {
  [ -f Cargo.toml ] || return 1
  awk '
    /^\[/ { in_package = ($0 ~ /^\[package\][ \t]*(#.*)?$/) }
    in_package && $1 == "name" && $2 == "=" {
      gsub(/^[ \t]*name[ \t]*=[ \t]*"/, ""); gsub(/".*$/, ""); print; exit
    }
  ' Cargo.toml
}

# The top-level "name" of package.json: the first key indented by exactly
# two spaces, which is how npm itself writes the manifest. Nested objects
# (`repository`, `bin`, ...) are indented deeper and never match.
npm_name_from_manifest() {
  [ -f "$NPM_PACKAGE_JSON" ] || return 1
  sed -n 's/^  "name"[ \t]*:[ \t]*"\([^"]*\)".*/\1/p' "$NPM_PACKAGE_JSON" | head -n 1
}

check_crates_io() {
  local crate_name status

  printf 'crates.io (trusted publishing):\n'

  crate_name=$(crate_name_from_manifest) || crate_name=''
  if [ -z "$crate_name" ]; then
    skip 'no Cargo.toml [package] name -- crates.io publishing is not configured'
    crates_verdict='skipped'
    return 0
  fi

  status=$(http_status -A "$CURL_USER_AGENT" "$CRATES_API/api/v1/crates/${crate_name}")
  case "$status" in
    200)
      pass "${crate_name} exists on crates.io -- the release job publishes it with trusted publishing (OIDC, no long-lived token)"
      crates_verdict='ok'
      ;;
    404)
      problem bootstrap "${crate_name} is not on crates.io yet -- trusted publishing cannot create a crate, so crates.io publication is skipped. Publish the first version once with: package-registry-manager setup --registry crates-io --execute"
      crates_verdict='bootstrap'
      ;;
    '')
      problem unknown "crates.io unreachable while checking whether ${crate_name} exists -- crates.io publication is skipped this run"
      crates_verdict='unknown'
      ;;
    *)
      problem unknown "crates.io answered ${status} while checking whether ${crate_name} exists (no verdict) -- crates.io publication is skipped this run"
      crates_verdict='unknown'
      ;;
  esac
  return 0
}

check_npm() {
  local package_name encoded status

  printf 'npm (trusted publishing):\n'

  package_name=$(npm_name_from_manifest) || package_name=''
  if [ -z "$package_name" ]; then
    skip "no package name in ${NPM_PACKAGE_JSON} -- npm publishing is not configured"
    npm_verdict='skipped'
    return 0
  fi

  # Scoped names (@scope/name) address the registry document as @scope%2Fname.
  encoded="${package_name//\//%2F}"
  status=$(http_status -H 'Accept: application/vnd.npm.install-v1+json' "$NPM_REGISTRY/${encoded}")
  case "$status" in
    200)
      pass "${package_name} exists on npm -- the release job publishes it with trusted publishing (OIDC, no NPM_TOKEN)"
      npm_verdict='ok'
      ;;
    404)
      problem bootstrap "${package_name} is not on npm yet -- trusted publishing cannot create a package, so npm publication is skipped. Publish the first version once with: package-registry-manager setup --registry npm --execute"
      npm_verdict='bootstrap'
      ;;
    '')
      problem unknown "the npm registry was unreachable while checking whether ${package_name} exists -- npm publication is skipped this run"
      npm_verdict='unknown'
      ;;
    *)
      problem unknown "the npm registry answered ${status} while checking whether ${package_name} exists (no verdict) -- npm publication is skipped this run"
      npm_verdict='unknown'
      ;;
  esac
  return 0
}

check_docker_hub() {
  local image="${DOCKERHUB_IMAGE:-}"
  local username="${DOCKERHUB_USERNAME:-}"
  local token="${DOCKERHUB_TOKEN:-}"

  printf 'Docker Hub:\n'

  if [ -z "$image" ]; then
    skip 'DOCKERHUB_IMAGE is not set -- Docker publishing is disabled (the release workflow disables it with the same condition)'
    docker_verdict='skipped'
    return 0
  fi

  if [ -z "$username" ] || [ -z "$token" ]; then
    problem refused "DOCKERHUB_IMAGE is set (${image}) but DOCKERHUB_USERNAME or DOCKERHUB_TOKEN is missing -- docker-publish would fail at login"
    docker_verdict='refused'
    return 0
  fi

  # The token request below is only a means to the write probe: as measured in
  # issue #167 the endpoint hands out 200 + a token for any scope without
  # proving the scope can be granted, so its answer proves nothing.
  local auth_body payload registry_token
  auth_body=$(http -u "$username:$token" \
    "$DOCKER_AUTH/token?service=registry.docker.io&scope=repository:${image}:pull,push")
  payload="${auth_body%"${NEWLINE}"*}"
  registry_token=$(printf '%s' "$payload" | sed -n 's/.*"token" *: *"\([^"]*\)".*/\1/p')
  if [ -z "$registry_token" ]; then
    problem unknown 'Docker Hub auth endpoint did not return a usable token'
    docker_verdict='unknown'
    return 0
  fi

  local headers status location
  headers=$(curl -sS --max-time "$CURL_TIMEOUT" -D - -o /dev/null \
    -X POST -H "Authorization: Bearer $registry_token" \
    "$DOCKER_REGISTRY/v2/${image}/blobs/uploads/" 2>/dev/null)
  if [ -z "$headers" ]; then
    problem unknown 'Docker Hub registry unreachable during the write probe'
    docker_verdict='unknown'
    return 0
  fi
  status=$(printf '%s\n' "$headers" | awk 'NR==1{gsub(/\r/,"");print $2}')
  location=$(printf '%s\n' "$headers" | awk 'tolower($1)=="location:"{gsub(/\r/,"");print $2; exit}')

  case "$status" in
    202)
      # Cancel the opened upload session so nothing is stored.
      if [ -n "$location" ]; then
        curl -sS --max-time "$CURL_TIMEOUT" -o /dev/null -X DELETE \
          -H "Authorization: Bearer $registry_token" "$location" 2>/dev/null || true
      fi
      pass "Docker Hub accepted a blob-upload write for ${image} (202; upload session cancelled)"
      docker_verdict='ok'
      ;;
    401 | 403)
      problem refused "Docker Hub refused the write for ${image} (${status}) -- the token cannot push this repository; the login the publishing jobs run would still have succeeded"
      docker_verdict='refused'
      ;;
    404)
      problem refused "Docker Hub reports ${image} as unknown (404) -- check DOCKERHUB_IMAGE and DOCKERHUB_USERNAME"
      docker_verdict='refused'
      ;;
    429)
      problem unknown 'Docker Hub rate-limited the write probe (429)'
      docker_verdict='unknown'
      ;;
    *)
      problem unknown "Docker Hub answered ${status:-no status} to the write probe (no verdict on the credential)"
      docker_verdict='unknown'
      ;;
  esac

  return 0
}

# In release mode a refused credential is an error (somebody has to fix a
# secret); a missing package or an unknown verdict is a warning. Report mode
# only ever warns.
emit_annotations() {
  [ -n "$problems" ] || return 0
  printf '%s\n' "$problems" | while IFS='|' read -r verdict message; do
    [ -n "$verdict" ] || continue
    level='warning'
    if [ "$MODE" = 'release' ] && [ "$verdict" = 'refused' ]; then
      level='error'
    fi
    printf '::%s::release-preflight: %s\n' "$level" "$message"
  done
}

write_outputs() {
  [ -n "${GITHUB_OUTPUT:-}" ] || return 0
  {
    printf 'crates=%s\n' "$crates_verdict"
    printf 'npm=%s\n' "$npm_verdict"
    printf 'docker=%s\n' "$docker_verdict"
  } >> "$GITHUB_OUTPUT"
}

append_summary() {
  [ -n "${GITHUB_STEP_SUMMARY:-}" ] || return 0
  {
    printf '### Release preflight (%s mode)\n\n' "$MODE"
    printf '| registry | verdict | publishes this run |\n| --- | --- | --- |\n'
    printf '| crates.io | %s | %s |\n' "$crates_verdict" "$([ "$crates_verdict" = ok ] && echo yes || echo no)"
    printf '| npm | %s | %s |\n' "$npm_verdict" "$([ "$npm_verdict" = ok ] && echo yes || echo no)"
    printf '| Docker Hub | %s | %s |\n' "$docker_verdict" "$([ "$docker_verdict" = ok ] && echo yes || echo no)"
    if [ -n "$problems" ]; then
      printf '\n'
      printf '%s\n' "$problems" | while IFS='|' read -r verdict message; do
        [ -n "$verdict" ] && printf -- '- **%s**: %s\n' "$verdict" "$message"
      done
    fi
  } >> "$GITHUB_STEP_SUMMARY"
}

check_crates_io
check_npm
check_docker_hub

printf '\nRelease preflight: crates=%s npm=%s docker=%s\n' \
  "$crates_verdict" "$npm_verdict" "$docker_verdict"

write_outputs
emit_annotations
append_summary

if [ -n "$problems" ]; then
  if [ "$MODE" = 'release' ]; then
    printf 'Release mode: only registries with verdict ok publish this run; the others are skipped and do not block the rest of the release.\n'
  else
    printf 'Report mode: the findings above are advisory -- pull requests may come from forks without publishing secrets.\n'
  fi
fi

exit 0
