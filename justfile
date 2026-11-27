# Run every recipe from the repo root
set working-directory := '.'

import 'just/common.just'

# Checks, formatting, dependencies and profiling: `just dev`.
mod dev 'just/dev.just'

# Scaffolded demo catalogs and gallery's own chrome: `just demo`.
mod demo 'just/demo.just'

# Cutting and checking a release: `just release`.
mod release 'just/release.just'
