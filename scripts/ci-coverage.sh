#!/usr/bin/env bash
# Surface the coverage table in the GitHub UI with no third-party service or action: append it to the
# Actions run summary, and — on a pull request — post or update one sticky comment. `gh` and the job
# token are GitHub-native. Arg: $1 = a file holding the `cargo llvm-cov report` table.
# Env: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_STEP_SUMMARY, PR (empty when the run isn't a pull request).
set -euo pipefail

table="$1"
marker='<!-- gallery-coverage -->'
report="$(
    printf '### Coverage\n\n```\n'
    cat "$table"
    printf '```\n'
)"

# Job summary — shown on every run, main and PR alike.
printf '%s\n' "$report" >>"$GITHUB_STEP_SUMMARY"

# Sticky PR comment — one comment, edited in place. A fork PR's token is read-only, so a failure here
# must not fail the build.
[ -n "${PR:-}" ] || exit 0
body="$(mktemp)"
printf '%s\n%s\n' "$marker" "$report" >"$body"
id="$(gh api "repos/$GITHUB_REPOSITORY/issues/$PR/comments" \
    --jq "map(select(.body | startswith(\"$marker\"))) | .[0].id // empty" 2>/dev/null || true)"
if [ -n "$id" ]; then
    gh api -X PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$id" -F body=@"$body" >/dev/null \
        || echo "coverage: comment update skipped"
else
    gh api -X POST "repos/$GITHUB_REPOSITORY/issues/$PR/comments" -F body=@"$body" >/dev/null \
        || echo "coverage: comment post skipped (fork PRs have a read-only token)"
fi
