use super::*;

/// The one test that asserts what a capture *looks like*;
/// the rest check that a value arrived.
///
/// Through [`read_recipe`] rather than hand-built [`Shot`]s,
/// so it covers what someone writes.
///
/// Comparable between machines only because the tests pin
/// a software rasteriser.
///
/// `UPDATE_SNAPSHOTS=1` takes an intended change.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn the_reference_images_match_what_the_recipe_renders() {
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-reference");
    let recipe = Utf8Path::new("tests/reference.toml");
    let capture = read_recipe(recipe, Some(&out)).expect("the reference recipe");
    render_with_globals(
        &reference_scenes(),
        None,
        &reference_plays(),
        Renderer::Glow,
        &|_: &egui::Context| {},
        &capture,
        LogMode::Off,
    )
    .expect("every reference shot renders");

    // Collected, not asserted one at a time: a layout change moves several images, and seeing all
    // of them beats fixing them one run apiece.
    let mut results = egui_kittest::SnapshotResults::new();
    for path in capture
        .shots
        .iter()
        .filter_map(|shot| shot.out.as_ref())
        .chain(capture.sheet.as_ref())
    {
        let name = path.file_stem().expect("a PNG filename");
        let png = std::fs::read(path).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        results.add(egui_kittest::try_image_snapshot(&image, name));
    }
    results.unwrap();
}
