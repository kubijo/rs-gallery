#!/usr/bin/env bash
# `just demo-<variant>` runs the consumer init command against THIS checkout: scaffold template/ straight
# into a git-ignored demo-<variant>/ with --no-workspace (so it never touches this repo's own workspace),
# repoint the gallery deps at the working tree, and run it. Any trailing arguments go to the gallery
# binary (e.g. --hot). Re-run (or delete the dir) to reset; the build cache lives in target/demo-<variant>
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
demo="$repo/demo-$variant"

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
cargo generate --path "$repo/template" --destination "$repo" --name "demo-$variant" --no-workspace \
    --vcs none --silent --define gallery_git=LOCAL --define scene_globs='*.scene.rs' \
    --define title="gallery demo ($variant)"

# Repoint the git-dep placeholders at this working tree so the demo exercises local changes.
sed -i \
    -e "s#gallery = { git = \"LOCAL\" }#gallery = { path = \"$repo\" }#" \
    -e "s#gallery-build = { git = \"LOCAL\", package = \"gallery-build\" }#gallery-build = { path = \"$repo/gallery-build\" }#" \
    "$demo/Cargo.toml"

if [ "$variant" = femtovg ]; then
    # Switch the host to the glow backend and add the femtovg scene + its dependencies. The pins are
    # deliberate: femtovg 0.20.4 (and the matching glow 0.16, which the scene uses to name femtovg's
    # framebuffer type) sit against eframe 0.35's glow 0.17, so the demo builds two incompatible glow
    # versions into one binary. Nothing shares a glow type across the boundary — gallery hands the scene
    # a raw C proc-address loader and femtovg builds its own glow from it — so that mismatch surviving is
    # the proof of renderer (and glow-version) independence.
    sed -i 's/gallery::Renderer::Wgpu/gallery::Renderer::Glow/' "$demo/main.rs"
    sed -i -e '/^egui = "0.35"$/a femtovg = "0.20.4"' -e '/^egui = "0.35"$/a glow = "0.16"' \
        "$demo/Cargo.toml"
    cp "$repo/scripts/offscreen.scene.rs" "$demo/offscreen.scene.rs"

    # Surface the mismatch before launching: every glow version the dependency graph resolved.
    echo "demo: glow versions in this binary, bridged only by the GL loader:" >&2
    cargo tree --manifest-path "$demo/Cargo.toml" 2>/dev/null \
        | grep -oE '\bglow v[0-9]+\.[0-9]+\.[0-9]+' | sort -u | sed 's/^/  /' >&2
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}/demo-$variant"
exec cargo run --manifest-path "$demo/Cargo.toml" -- "$@"
