#!/usr/bin/env bash
# `just demo-<variant>` runs the consumer init command against THIS checkout: scaffold template/ straight
# into a git-ignored .tmp/demo-<variant>/ with --no-workspace (so it never touches this repo's own workspace),
# repoint the gallery deps at the working tree, and run it. Any trailing arguments go to the gallery
# binary (e.g. --hot). Re-run (or delete the dir) to reset; the build cache lives in .tmp/target/demo-<variant>
# so the per-run wipe stays cheap.
#
# Two variants prove the shell is renderer-independent:
#   wgpu     — the default eframe backend; the bundled pure-egui scenes.
#   femtovg  — the glow (OpenGL) backend, plus a scene that draws with femtovg into gallery's offscreen
#              FBO. The same pure-egui scenes render unchanged under OpenGL, and the femtovg scene drives
#              `ctx.offscreen`, so together they exercise both halves of renderer independence.
set -euo pipefail

variant="${1:-}"
case "$variant" in
wgpu | femtovg) shift ;;
*)
    echo "demo: usage: demo.sh <wgpu|femtovg> [gallery args...]" >&2
    exit 2
    ;;
esac

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
demo="$repo/.tmp/demo-$variant"

if ! command -v cargo-generate >/dev/null 2>&1; then
    # Single quotes are deliberate: the backticks are literal hint punctuation, not a command
    # substitution — so SC2016 (no expansion in single quotes) is expected here, not a bug.
    # shellcheck disable=SC2016
    echo 'demo: cargo-generate not on PATH — enter the dev shell, or run `cargo install cargo-generate`' >&2
    exit 1
fi

# The same `cargo generate … --name <dir> --no-workspace` a consumer runs, here with --path for the
# local template. --no-workspace is what stops it from splicing the dir into this repo's workspace; the
# template's own [workspace] then keeps the instance standalone.
rm -rf "$demo"
mkdir -p "$repo/.tmp"
cargo generate --path "$repo/template" --destination "$repo/.tmp" --name "demo-$variant" --no-workspace \
    --vcs none --silent --define gallery_git=LOCAL --define scene_globs='*.scene.rs' \
    --define title="gallery demo ($variant)"

# Repoint the git-dep placeholders at this working tree so the demo exercises local changes.
sed -i \
    -e "s#gallery = { git = \"LOCAL\" }#gallery = { path = \"$repo\" }#" \
    -e "s#gallery-build = { git = \"LOCAL\", package = \"gallery-build\" }#gallery-build = { path = \"$repo/gallery-build\" }#" \
    "$demo/Cargo.toml"

if [ "$variant" = femtovg ]; then
    # The version clash is the point. femtovg 0.20.4 needs glow 0.16 — which the scene names too,
    # for femtovg's framebuffer type — while eframe 0.36 brings 0.17. Nothing crosses between them
    # but a raw C proc-address loader, so two glow versions in one binary prove a scene's renderer
    # is its own.
    #
    # Anchored on the section header rather than a dependency line, which comes and goes.
    sed -i 's/gallery::Renderer::Wgpu/gallery::Renderer::Glow/' "$demo/main.rs"
    sed -i -e '/^\[dependencies\]/a femtovg = "0.20.4"' -e '/^\[dependencies\]/a glow = "0.16"' \
        "$demo/Cargo.toml"
    cp "$repo/scripts/offscreen.scene.rs" "$demo/offscreen.scene.rs"

    # Surface the mismatch before launching: every glow version the dependency graph resolved.
    echo "demo: glow versions in this binary, bridged only by the GL loader:" >&2
    cargo tree --manifest-path "$demo/Cargo.toml" 2>/dev/null \
        | grep -oE '\bglow v[0-9]+\.[0-9]+\.[0-9]+' | sort -u | sed 's/^/  /' >&2
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/.tmp/target}/demo-$variant"

# Built before it is run so the compile can go quiet without silencing the run itself:
# a headless `--render` prints the paths it wrote, which a screen of `Compiling` lines buries.
# The stand-in line is for a terminal only — a CI log has no cursor to move back over.
building="demo: building demo-$variant…"
if [ -t 2 ]; then printf '%s' "$building" >&2; fi
cargo build --quiet --manifest-path "$demo/Cargo.toml"
if [ -t 2 ]; then printf '\r%*s\r' "${#building}" '' >&2; fi

exec cargo run --quiet --manifest-path "$demo/Cargo.toml" -- "$@"
