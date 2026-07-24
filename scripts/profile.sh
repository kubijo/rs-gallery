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

for tool in cargo-generate samply; do
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

export CARGO_TARGET_DIR="$repo/target/profile"
# The launcher resolves globs against the config dir before handing them to the build; matching that
# exactly means its own `build_lib` is a cache hit rather than a cargo run inside the recording.
demo_abs=$(cd "$demo" && pwd -P)
export GALLERY_SCENE_GLOBS="$demo_abs/*.scene.rs"
(cd "$demo" && cargo build --profile profiling)

args=(--frames "$frames")
[ -n "$scene" ] && args+=(--scene "$scene")

echo "profile: recording $frames frames${scene:+ of $scene} → reports/$report"
samply record --save-only -o "$out/profile.json.gz" -- \
    "$CARGO_TARGET_DIR/profiling/gallery" "${args[@]}"

echo
echo "profile: saved $out/profile.json.gz"
echo "  view:  samply load $out/profile.json.gz"
