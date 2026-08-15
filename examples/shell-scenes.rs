//! Gallery's own components, browsed in gallery: `just shell-scenes`.
//!
//! They live inside the crate and reach the shell through [`Linked`](gallery::Linked), as a host
//! that links its own scenes does. No dylib, so nothing here reloads — `--hot` is only the subject.

fn main() -> gallery::eframe::Result {
    gallery::run(
        "gallery — shell scenes",
        gallery::Linked,
        gallery::Settings::new(gallery::Renderer::Wgpu),
        |_| {},
    )
}
