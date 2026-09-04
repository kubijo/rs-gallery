//! The gallery host. `cargo run` opens the window; `cargo run -- --hot` adds live reload.
//! What it shows is configured in `gallery.toml`.

fn main() -> gallery::eframe::Result {
    let window_icon =
        gallery::eframe::icon_data::from_png_bytes(include_bytes!("assets/window-icon.png"))
            .expect("the bundled demo window icon is a valid PNG");
    gallery::launch!(
        |_| {},
        gallery::Settings::new(gallery::Renderer::Wgpu)
            .window_icon(window_icon)
            .controls_default_width(260.0)
            // `true` folds every sidebar folder; a list folds those top-level ones.
            .collapsed(true)
    )
}
