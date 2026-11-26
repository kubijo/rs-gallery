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

# Exactly 1, and every failing play in the report: a crash on the way exits otherwise
# and leaves no report, where a play reported as failed was caught as it should be.
expected=$(grep -c '^name = "fails-' "$repo/scripts/plays.toml")
reported=0
if [ -f "$out/report.json" ]; then
    reported=$(grep -c '"reason":' "$out/report.json" || true)
fi

if [ "$status" -ne 1 ] || [ "$reported" -ne "$expected" ]; then
    echo >&2
    echo "demo-plays: expected exit 1 and each failure reported" >&2
    echo "  exit $status; failures reported: $reported of $expected" >&2
    exit 1
fi
"$repo/scripts/check-tree.sh" "$out" scripts/plays.dirspec.toml
echo >&2
echo "demo-plays: as meant, the passing plays passed and the failing plays failed" >&2

# The same catalog's own tests: each `/// Fails:` above fails its trial,
# a panic only its own, and every other scene and play passes.
# A crash would leave no summary line to count.
printf '\n[tests]\noutput = "%s"\n' "$out/tests" >>"$repo/.tmp/demo-wgpu/gallery.toml"
tests_status=0
tests=$(cd "$repo/.tmp/demo-wgpu" && cargo test --quiet --test scenes 2>&1) || tests_status=$?
# As it is now, of every test: those below run only some again. CI uploads it, whatever comes of the checks.
mkdir "$out/report"
cp -r "$out/tests/index.html" "$out/tests/app_gallery" "$out/report" || true
failed=$(grep -oE '[0-9]+ failed' <<<"$tests" | grep -oE '[0-9]+' || echo none)
meant=$(grep -c '^/// Fails:' "$repo/scripts/plays.scene.rs")
if [ "$tests_status" -eq 0 ] || [ "$failed" != "$meant" ]; then
    echo "$tests" >&2
    echo "demo-plays: expected the catalog's tests to fail exactly $meant trials; exit $tests_status, failed: $failed" >&2
    exit 1
fi

# There is a result of every one of them, and the report has it, as passed or failed as the terminal has it.
"$repo/scripts/check-tree.sh" "$out/tests" scripts/plays-tests.dirspec.toml
for verdict in passed failed; do
    ran=$(grep -oE "[0-9]+ $verdict" <<<"$tests" | grep -oE '[0-9]+' || echo none)
    kept=$(find "$out/tests/app_gallery/results-wgpu" -mindepth 2 -name '*.json' -exec jq -r .verdict {} + \
        | grep -cx "$verdict" || true)
    reported=$({ grep -oE "<(details|span) class=\"(plain )?trial $verdict\"" "$out/tests/index.html" || true; } | wc -l)
    if [ "$kept" != "$ran" ] || [ "$reported" != "$ran" ]; then
        echo "demo-plays: $ran tests $verdict; results of $kept, $reported in the report" >&2
        exit 1
    fi
done

# A panic the play catches is reported at its own line, and is no end of the process.
if grep -q 'panicked after it was drawn' <<<"$tests" \
    || ! grep -q '[0-9] .*panic!("the frame' <<<"$tests"; then
    echo "$tests" >&2
    echo "demo-plays: expected the frame's end to fail its play at the line that panics" >&2
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

# A panic in a paint callback, which the callback alone could catch,
# says whose it was before the process ends.
ended_status=0
ended=$(cd "$repo/.tmp/demo-wgpu" && DEMO_CALLBACK_PANICS=1 cargo test --quiet --test scenes -- paints_with_a_callback 2>&1) || ended_status=$?
if [ "$ended_status" -eq 0 ] || ! grep -q 'the callback panics' <<<"$ended" \
    || ! grep -q 'scene .Calls back. panicked after it was drawn' <<<"$ended"; then
    echo "$ended" >&2
    echo "demo-plays: expected the panicking callback's scene and message; exit $ended_status" >&2
    exit 1
fi
if ! grep -A1 '<details class="trial unfinished"' "$out/tests/index.html" | grep -q 'paints_with_a_callback'; then
    echo "demo-plays: expected the report to have the test that ended its process as unfinished" >&2
    exit 1
fi

echo "demo-plays: as meant, the catalog's tests failed only the $meant trials meant to fail" >&2
