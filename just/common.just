# Settings and helpers for every justfile here; modules don't inherit them, so each one imports this.

# A bare `just`, or `just <module>`, lists recipes instead of running the first one.
set default-list

# Run a tool in the Nix dev shell, or directly when it is already on PATH.
[private]
in-shell +cmd:
    #!/usr/bin/env bash
    set -euo pipefail
    set -- {{ cmd }}
    if command -v "$1" >/dev/null 2>&1; then exec "$@"; fi
    exec nix develop --command "$@"
