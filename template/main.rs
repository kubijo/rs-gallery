//! The gallery host. `cargo run` opens the window; `cargo run -- --hot` adds live reload.
//! What it shows is configured in `gallery.toml`.

fn main() -> gallery::eframe::Result {
    gallery::launch!(|_| {}, gallery::Settings::new(gallery::Renderer::Wgpu))
}
