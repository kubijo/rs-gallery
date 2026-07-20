//! Discover the scene files matched by `gallery.toml`'s globs (passed via env by the launcher).

fn main() {
    gallery_build::discover_from_env();
}
