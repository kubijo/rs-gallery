# Demos: scaffolded catalogs run against this checkout, and gallery's own chrome. Recipes run from the repo root.
set working-directory := '..'

import 'common.just'

# Scaffold + run the wgpu demo (default backend, pure-egui scenes).
wgpu *args:
    @scripts/demo.sh wgpu {{ args }}

# Scaffold + run the glow/femtovg demo into .tmp/demo-femtovg/.
femtovg *args:
    @scripts/demo.sh femtovg {{ args }}

# Run the demo's plays through its scenes dylib, plus one that fails on purpose, into .tmp/plays/.
plays:
    @scripts/demo-plays.sh

# Record a scene headlessly into .tmp/record/<backend>/, e.g. `just demo::record femtovg mp4`.
record backend='wgpu' format='apng' scene='orbit' rate='60:2':
    @scripts/demo.sh {{ backend }} --scene {{ scene }} \
        --render .tmp/record/{{ backend }}/{{ scene }}.png --record {{ rate }} --record-format {{ format }}

# Record the studio tour, its input shown, and the checks tour, each in every video format, on both backends.
recordings:
    @scripts/demo-recordings.sh

# Browse the shell's own components — every state of the chrome gallery draws around a scene.
shell-scenes *args:
    @cargo run --example shell-scenes --features shell-scenes {{ args }}
