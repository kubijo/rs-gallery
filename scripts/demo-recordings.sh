#!/usr/bin/env bash

# `just demo::recordings`: on both backends, record the studio and checks tours
# in every video format, and check each file holds every frame its play drew
# and pauses at the play's step ends.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
recipe="$repo/scripts/recordings.toml"

problems=()
for backend in wgpu femtovg; do
    out="$repo/.tmp/recordings/$backend"
    rm -rf "$out"
    if ! "$repo/scripts/demo.sh" "$backend" --capture "$recipe" --out "$out"; then
        problems+=("$backend: the capture failed")
        continue
    fi
    if ! "$repo/scripts/check-tree.sh" "$out" scripts/recordings.dirspec.toml; then
        problems+=("$backend: not the files it should leave")
        continue
    fi
    for tour in studio-tour checks-tour; do
        played=$(jq --arg name "$tour" '.shots[] | select(.name == $name) | .play.frames' \
            "$out/report.json")
        for format in apng mp4; do
            video="$out/$tour.$format"
            read -r recorded held < <(jq -r --arg name "$tour" --arg format "$format" \
                '.shots[] | select(.name == $name) | .videos[] | select(.format == $format)
                    | "\(.frames) \(.held)"' "$out/report.json") || true
            if ! [[ ${recorded:-} =~ ^[0-9]+$ ]] || [ "$recorded" != "$played" ]; then
                problems+=("$backend: $video holds ${recorded:-no} frames of the ${played:-?} its play drew")
            elif [ "${held:-0}" -eq 0 ]; then
                problems+=("$backend: $video never paused at a step's end")
            fi
        done
    done
done

if [ "${#problems[@]}" -gt 0 ]; then
    printf 'demo-recordings: %s\n' "${problems[@]}" >&2
    exit 1
fi
echo "demo-recordings: both tours recorded in every format on both backends" >&2
