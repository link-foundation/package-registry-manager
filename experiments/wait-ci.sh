#!/usr/bin/env bash
# Wait until every workflow run for a commit has completed, then print them.
# Usage: experiments/wait-ci.sh <sha>
set -u
sha=${1:?sha}
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
