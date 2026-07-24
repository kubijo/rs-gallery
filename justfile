# Run every recipe from the repo root
set working-directory := '.'

# Bare `just` lists recipes instead of running the first one.
[private]
default:
    @just --justfile {{ justfile() }} --list

# Run a tool in the Nix dev shell, or directly when it is already on PATH.
[private]
in-shell +cmd:
    #!/usr/bin/env bash
    set -euo pipefail
    set -- {{ cmd }}
    if command -v "$1" >/dev/null 2>&1; then exec "$@"; fi
    exec nix develop --command "$@"

# Scaffold + run the wgpu demo (default backend, pure-egui scenes).
demo-wgpu *args:
    @scripts/demo.sh wgpu {{ args }}

# Scaffold + run the glow/femtovg demo into demo-femtovg/.
demo-femtovg *args:
    @scripts/demo.sh femtovg {{ args }}

# Record a CPU profile of SCENE (a key fragment, e.g. `orbit`) into reports/REPORT.
profile SCENE="" REPORT="00-latest" FRAMES="600":
    @scripts/profile.sh {{ SCENE }} {{ REPORT }} {{ FRAMES }}

# Reformat the whole repo (nix, markdown, shell, rust, and SVG via treefmt).
format *args:
    @just in-shell repofmt {{ args }}

# Validate the crate: formatting, repo lint, clippy, and tests under coverage.
validate:
    @just in-shell validate

# Report direct dependencies with a newer version available.
outdated:
    @just in-shell cargo outdated --root-deps-only

# Dependency audit — cargo-deny: advisories (RustSec) + license/ban/source policy (deny.toml).
audit:
    @just in-shell cargo deny check

# Build the API docs.
docs *args:
    @just in-shell cargo doc --no-deps {{ args }}
