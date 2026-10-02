#!/usr/bin/env bash
# Wait until every workflow run for a commit has completed, then print them.
# Usage: experiments/wait-ci.sh <sha>
set -u
# gh run list --commit only matches a full SHA, so resolve abbreviations.
sha=$(git rev-parse --verify "${1:?sha}^{commit}") || exit 1
while true; do
  out=$(gh run list --repo link-foundation/package-registry-manager --commit "$sha" \
    --json status,conclusion,workflowName,databaseId \
    --jq '.[] | "\(.workflowName)|\(.status)|\(.conclusion)|\(.databaseId)"')
  if [ -n "$out" ] && ! grep -qv '|completed|' <<<"$out"; then
    echo "$out"
    exit 0
  fi
  sleep 60
done
