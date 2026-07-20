# TODO

## Publishing to crates.io (deferred)

Not required while the repo is source-only — each consumer's Cargo fetches the crates themselves. Do this only before an
actual `cargo publish`. Research: crates.io Trusted Publishing (RFC 3691), Cargo publishing reference.

### Manifests

- [ ] Flip `publish = false` on all three crates (`gallery`, `gallery-macros`, `gallery-build`).
- [ ] Add `repository = "https://github.com/kubijo/rs-gallery"` to `gallery-macros` + `gallery-build` (`gallery` already
  has it). Recommended metadata, and it's the repo a Trusted Publisher binds to; `license`/`description`/`readme`/
  `keywords`/`categories` are already set.

### Publish order (path-dep rule)

- [ ] crates.io forbids publishing a crate with a path-only dependency, so publish `gallery-macros` + `gallery-build`
  first, then `gallery` (it path-depends on `gallery-macros`). `release-plz` / `cargo-release` handle this ordering.

### Template

- [ ] `template/` depends on `gallery = { git = … }`; add a registry-dep variant (`gallery = "0.1"`) for post-publish
  consumers, or keep git while pre-release.

### Trusted Publishing (the only provenance mechanism crates.io has)

- [ ] Register a Trusted Publisher per crate on crates.io: owner `kubijo`, repo `rs-gallery`, workflow `release.yml`,
  environment `release`.
- [ ] Add `.github/workflows/release.yml`: tag-triggered, `permissions: id-token: write`,
  `rust-lang/crates-io-auth-action` → `cargo publish`. No long-lived `CRATES_IO_TOKEN` secret.
- [ ] crates.io account needs a verified email.
- Artifact provenance / sigstore attestation is explicitly out of scope on crates.io (RFC 3691) — nothing to set up
  there.

### Tagging

- [ ] Tag every release for `version ↔ commit` auditability. Workspace convention: `<pkg>-v<version>` (e.g.
  `gallery-v0.1.0`), or a single `v0.1.0` if the three version in lockstep.
- [ ] Consider `release-plz` — release PR from the Keep-a-Changelog `CHANGELOG.md` → tag → publish.

## Later

- [ ] Auto-re-sync an instance's scaffolded files (`justfile` / `build.rs` / `main.rs`) from upstream as the template
  evolves. Hard to do safely — it must not clobber consumer edits — so `just update`'s "you're behind, here's what
  changed" is the lightweight stand-in for now.
