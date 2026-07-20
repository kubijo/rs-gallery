//! Bundled glyph-fallback fonts.
//!
//! egui's default faces miss common non-text glyphs — arrows, math operators,
//! geometric and technical symbols — and render them as tofu (□).
//!
//! We append a few Noto faces to the fallback chain of both font families
//! so those glyphs resolve. They are fallbacks only: the default proportional
//! and monospace faces stay first, so the shell's look is unchanged and the Noto
//! faces are consulted just for glyphs the defaults lack.
//!
//! CJK and emoji are out of scope — each is a multi-megabyte font
//! — so a catalog that renders that text adds its own via the host `setup` closure.
//!
//! The fonts are SIL OFL 1.1; the license and per-face copyright notices live in `fonts/noto/OFL.txt`.

use std::sync::Arc;

/// Name → bytes for each bundled fallback face, in fallback priority.
const FALLBACKS: &[(&str, &[u8])] = &[
    (
        "NotoSans",
        include_bytes!("../fonts/noto/NotoSans-Regular.ttf"),
    ),
    (
        "NotoSansSymbols",
        include_bytes!("../fonts/noto/NotoSansSymbols-Regular.ttf"),
    ),
    (
        "NotoSansSymbols2",
        include_bytes!("../fonts/noto/NotoSansSymbols2-Regular.ttf"),
    ),
    (
        "NotoSansMath",
        include_bytes!("../fonts/noto/NotoSansMath-Regular.ttf"),
    ),
];

/// Append the bundled Noto faces to every font family's fallback chain, so glyphs egui's defaults lack
/// (arrows, math, symbols) resolve instead of rendering as tofu. Defaults stay first, so text that the
/// default faces already cover is untouched.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in FALLBACKS {
        fonts.font_data.insert(
            (*name).to_owned(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let chain = fonts.families.entry(family).or_default();
        chain.extend(FALLBACKS.iter().map(|(name, _)| (*name).to_owned()));
    }
    ctx.set_fonts(fonts);
}
