//! Discover the scene files matched by `gallery.toml`'s globs, which it reads itself —
//! so a bare `cargo build` compiles in the scenes a launcher run does.

fn main() {
    gallery_build::discover_from_env();
}
