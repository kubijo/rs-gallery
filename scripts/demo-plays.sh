#!/usr/bin/env bash
# `just demo::plays`: run plays through a real scenes dylib, those that pass and those that fail
# on purpose: in one capture, then as the catalog's tests, which alone compare snapshots.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$repo/.tmp/plays"
rm -rf "$out"

# A failed play does not stop the run, so the capture exits non-zero after every shot has run.
status=0
GALLERY_DEMO_SCENES="$repo/scripts/plays.scene.rs" \
    "$repo/scripts/demo.sh" wgpu --capture "$repo/scripts/plays.toml" --out "$out" || status=$?

missing=()
for shot in form-saved keyboard-tabs tasks-filtered tasks-sorted tasks-priority \
    tasks-scrolled tasks-cleared tasks-not-compared; do
    [ -f "$out/$shot.png" ] || missing+=("$shot")
done
unexpected=()
for png in "$out"/fails-*.png; do
    if [ -e "$png" ]; then
        unexpected+=("$(basename "$png" .png)")
    fi
done
# Exactly 1, and every failing play in the report: a crash on the way exits otherwise
# and leaves no report, where a play reported as failed was caught as it should be.
expected=$(grep -c '^name = "fails-' "$repo/scripts/plays.toml")
reported=0
if [ -f "$out/report.json" ]; then
    reported=$(grep -c '"reason":' "$out/report.json" || true)
fi

if [ "$status" -ne 1 ] || [ "${#missing[@]}" -gt 0 ] || [ "${#unexpected[@]}" -gt 0 ] \
    || [ "$reported" -ne "$expected" ]; then
    echo >&2
    echo "demo-plays: expected exit 1, every passing shot, no failing one, and each failure reported" >&2
    echo "  exit $status; missing: ${missing[*]:-none}; passed but should not: ${unexpected[*]:-none};" \
        "failures reported: $reported of $expected" >&2
    exit 1
fi
echo >&2
echo "demo-plays: as meant, the passing plays passed and the failing plays failed" >&2

# The same catalog's own tests: each `/// Fails:` above fails its trial,
# a panic only its own, and every other scene and play passes.
# A crash would leave no summary line to count.
tests_status=0
tests=$(cd "$repo/.tmp/demo-wgpu" && cargo test --quiet --test scenes 2>&1) || tests_status=$?
failed=$(grep -oE '[0-9]+ failed' <<<"$tests" | grep -oE '[0-9]+' || echo none)
meant=$(grep -c '^/// Fails:' "$repo/scripts/plays.scene.rs")
if [ "$tests_status" -eq 0 ] || [ "$failed" != "$meant" ]; then
    echo "$tests" >&2
    echo "demo-plays: expected the catalog's tests to fail exactly $meant trials; exit $tests_status, failed: $failed" >&2
    exit 1
fi

# Run again for a terminal to show in colour: what a failing snapshot looks like.
echo >&2
echo "demo-plays: the snapshots, each failing as meant" >&2
(cd "$repo/.tmp/demo-wgpu" && cargo test --quiet --test scenes -- reference >&2) || true

# A reference is taken when asked for, and then compared with.
compares=compares_with_a_reference_never_taken
for update in 1 0; do
    # Filtered by name, so the tests also pass when none has it.
    if ! taken=$(cd "$repo/.tmp/demo-wgpu" && UPDATE_SNAPSHOTS=$update cargo test --quiet --test scenes -- "$compares" 2>&1) \
        || ! grep -q '\b1 passed' <<<"$taken"; then
        echo "$taken" >&2
        echo "demo-plays: expected $compares to pass under UPDATE_SNAPSHOTS=$update" >&2
        exit 1
    fi
done

echo "demo-plays: as meant, the catalog's tests failed only the $meant trials meant to fail" >&2
