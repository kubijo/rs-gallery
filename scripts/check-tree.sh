#!/usr/bin/env bash
# `check-tree.sh <directory> <spec>`: check the directory against a dirspec file, showing the tree only if it's amiss.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

dirspec=(dirspec)
if ! command -v dirspec >/dev/null 2>&1; then
    dirspec=(nix develop --command dirspec)
fi
if ! tree=$("${dirspec[@]}" "$1" --spec "$2" 2>&1); then
    echo "$tree" >&2
    echo "check-tree: $1 is not as $2 has it" >&2
    exit 1
fi
