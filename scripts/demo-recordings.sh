#!/usr/bin/env bash

# `just demo::recordings`: on both backends, record an animated scene
# in every video format and a play with its input shown, and check
# each animation holds every frame its length asks for.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
recipe="$repo/scripts/recordings.toml"
# 60 fps for 2 s, as both orbit shots in the recipe ask.
frames=120

problems=()
for backend in wgpu femtovg; do
    out="$repo/.tmp/recordings/$backend"
    rm -rf "$out"
    if ! "$repo/scripts/demo.sh" "$backend" --capture "$recipe" --out "$out"; then
        problems+=("$backend: the capture failed")
        continue
    fi
    tour="$out/studio-tour.apng"
    held=$(jq '.shots[] | select(.name == "studio-tour") | .video.held' "$out/report.json")
    if [ ! -s "$tour" ]; then
        problems+=("$backend: no $tour")
    elif [ "${held:-0}" -eq 0 ]; then
        problems+=("$backend: $tour never paused at a step's end")
    fi
    for format in apng mp4; do
        video="$out/orbit-$format.$format"
        recorded=$(jq --arg name "orbit-$format" '.shots[] | select(.name == $name) | .video.frames' \
            "$out/report.json")
        if [ ! -s "$video" ]; then
            problems+=("$backend: no $video")
        elif [ "$recorded" != "$frames" ]; then
            problems+=("$backend: $video holds ${recorded:-no} frames, not $frames")
        fi
    done
done

if [ "${#problems[@]}" -gt 0 ]; then
    printf 'demo-recordings: %s\n' "${problems[@]}" >&2
    exit 1
fi
echo "demo-recordings: every format recorded all $frames frames on both backends" >&2
