#!/usr/bin/env bash

# Records a CPU profile of one scene into reports/<REPORT>/, by scaffolding the template into a
# git-ignored demo-profile/, building it with symbols, and running it under samply.
#
# The fixed frame count is what makes two runs comparable — profile before a change and after, into
# differently named reports, and the sample counts mean the same thing. It is also the only way to get
# any samples at all: a reactive shell nobody is touching renders nothing.

set -euo pipefail

scene="${1:-}"
report="${2:-00-latest}"
frames="${3:-600}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
demo="$repo/demo-profile"
out="$repo/reports/$report"

for tool in cargo-generate samply uv addr2line; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "profile: $tool not on PATH — enter the dev shell, or cargo install $tool" >&2
        exit 1
    fi
done

# samply samples through perf events, which the kernel gates on this.
paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 0)
if [ "$paranoid" -gt 1 ]; then
    echo "profile: perf_event_paranoid=$paranoid — samply needs 1 or lower to sample. Run:" >&2
    echo "    echo -1 | sudo tee /proc/sys/kernel/perf_event_paranoid" >&2
    exit 1
fi

# `00-latest` is the scratch name and gets replaced; a named report is something you meant to keep.
if [ -d "$out" ] && [ "$report" != "00-latest" ]; then
    echo "profile: reports/$report already exists — name it differently or remove it." >&2
    exit 1
fi

rm -rf "$demo" "$out"
mkdir -p "$out"

cargo generate --path "$repo/template" --destination "$repo" --name demo-profile --no-workspace \
    --vcs none --silent --define gallery_git=LOCAL --define scene_globs='*.scene.rs' \
    --define title='gallery profile'

sed -i \
    -e "s#gallery = { git = \"LOCAL\" }#gallery = { path = \"$repo\" }#" \
    -e "s#gallery-build = { git = \"LOCAL\", package = \"gallery-build\" }#gallery-build = { path = \"$repo/gallery-build\" }#" \
    "$demo/Cargo.toml"

# A debug build profiles the wrong thing and a plain release strips what samply needs to attribute
# samples. The demo carries its own `[workspace]`, so this table covers gallery as its path dep too.
cat >>"$demo/Cargo.toml" <<'TOML'

[profile.profiling]
inherits = "release"
debug = true
TOML

# Its own target dir, so a profiling build never evicts the dev cache, nested under whatever the
# environment already asked for — a globally exported `CARGO_TARGET_DIR` points off this disk on
# purpose. A `[build] target-dir` in `~/.cargo/config.toml` is invisible to the shell and gets
# overridden; reading it would take `cargo metadata --format-version 1 | jq -r .target_directory`.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}/profile"

# The launcher rebuilds the scenes dylib at startup, under its own profile and with globs it resolves
# against the config dir. Pre-building the same profile with the same globs makes that a cache hit,
# keeping a cold compile of every dependency out of the recording.
demo_abs=$(cd "$demo" && pwd -P)
export GALLERY_SCENE_GLOBS="$demo_abs/*.scene.rs"
(cd "$demo" && cargo build --profile profiling)

args=(--frames "$frames")
[ -n "$scene" ] && args+=(--scene "$scene")

echo "profile: recording $frames frames${scene:+ of $scene} → reports/$report"
samply record --save-only -o "$out/profile.json.gz" -- \
    "$CARGO_TARGET_DIR/profiling/gallery" "${args[@]}"

# Resolve addresses here, not on demand: samply saves the profile unsymbolicated, and the next build
# replaces the very binaries its addresses point into. Non-fatal under `set -e`, so a missing symbol
# file still leaves a recording that `samply load` can symbolicate for itself.
if ! (cd "$repo/tools" && uv run --frozen gallery-perf symbolicate "$out/profile.json.gz"); then
    echo "profile: symbolication failed — the recording itself is intact" >&2
fi

echo
echo "profile: saved $out/profile.json.gz"
echo "  view:     samply load $out/profile.json.gz"
echo "  analyse:  (cd tools && uv run gallery-perf analyze $out/profile.json.gz)"
