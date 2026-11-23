#!/usr/bin/env bash
# `just demo-plays`: run plays through a real scenes dylib in one capture,
# those that pass and those that fail on purpose. Gallery lays out what each came to.
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
    tasks-scrolled tasks-cleared; do
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
