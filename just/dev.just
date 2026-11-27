# Development: checks, formatting, dependencies and profiling. Recipes run from the repo root.
set working-directory := '..'

import 'common.just'

# Reformat the whole repo.
format *args:
    @nix run .#format -- {{ args }}

# Validate the crate: formatting, repo lint, clippy, tests under coverage,
# plays through a real scenes dylib, and recordings on both backends.
validate:
    @just in-shell validate
    @just demo::plays
    @just demo::recordings
    @if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then on='{{ BOLD + GREEN }}' off='{{ NORMAL }}'; fi; \
        printf '\n%s✓ validate passed%s: formatting, lint, clippy, tests, the demo plays and recordings\n' "${on:-}" "${off:-}" >&2; \
        printf '  the failures shown above are the plays meant to fail, and each failed as meant\n' >&2

# Take the current renders as the committed references in `tests/snapshots/` — look at them first.
snapshots:
    @UPDATE_SNAPSHOTS=1 just in-shell gallery-test the_reference_images_match

# Dependency audit — cargo-deny: advisories (RustSec) + license/ban/source policy (deny.toml).
audit:
    @just in-shell cargo deny check -D warnings -A no-license-field

# Build the API docs.
docs *args:
    @RUSTDOCFLAGS="-D warnings" just in-shell cargo doc --no-deps {{ args }}

# Report direct dependency updates against the documented exceptions in outdated.toml.
outdated:
    @just in-shell gallery-outdated

# Upgrade every dependency lock.
upgrade:
    @just dev::upgrade-nix
    @just dev::upgrade-cargo
    @just dev::upgrade-python

# Upgrade Nix inputs.
upgrade-nix *args:
    @nix flake update {{ args }}

# Upgrade Rust dependencies within their declared requirements.
upgrade-cargo *args:
    @just in-shell cargo update {{ args }}

# Upgrade every Python tool dependency allowed by tools/pyproject.toml.
upgrade-python *args:
    @just in-shell uv lock --project tools --upgrade {{ args }}

# Record a CPU profile of SCENE (a key fragment, e.g. `orbit`) into .tmp/reports/REPORT.
profile SCENE="" REPORT="00-latest" FRAMES="600":
    @scripts/profile.sh {{ SCENE }} {{ REPORT }} {{ FRAMES }}
