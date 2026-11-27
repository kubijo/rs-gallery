# Releases: cutting a tag, and checking one. Recipes run from the repo root.
set working-directory := '..'

import 'common.just'

# Cut a release: bump LEVEL (major|minor|patch), date the changelog, commit and tag — on confirmation.
tag LEVEL:
    @just in-shell gallery-release tag {{ LEVEL }}

# Check TAG against the version the manifests carry and the CHANGELOG documents, before cutting it.
check TAG:
    @just in-shell gallery-release check {{ TAG }}
