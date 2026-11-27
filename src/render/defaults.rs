//! How a catalog draws, records and reports unless a recipe, a shot or a flag
//! says otherwise: its `gallery.toml`'s `[render]`, `[record]` and `[plays]`.
//! Handed to what renders, so a test can render as any catalog would.

use super::{
    Corner, Glide, LogMode, VideoFormat, VideoFormats, check_frames, check_scale, parse_size,
};

/// Every default a catalog's `gallery.toml` sets.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct Defaults {
    pub(crate) render: RenderDefaults,
    pub(crate) record: RecordDefaults,
    pub(crate) steps: Option<bool>,
    pub(crate) log: Option<LogMode>,
}

/// `[render]`, checked as a shot's own would be.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct RenderDefaults {
    pub(crate) size: Option<egui::Vec2>,
    pub(crate) scale: Option<f32>,
    pub(crate) trim: Option<bool>,
    pub(crate) frames: Option<u32>,
}

impl Defaults {
    /// # Errors
    /// For a `[render]` value no shot could take.
    pub(crate) fn of(config: &gallery_build::Config) -> Result<Self, String> {
        let render = &config.render;
        let refused = |key: &str, e: String| format!("gallery.toml [render] {key}: {e}");
        let size = render
            .size
            .as_deref()
            .map(parse_size)
            .transpose()
            .map_err(|e| refused("size", e))?;
        if let Some(scale) = render.scale {
            check_scale(scale).map_err(|e| refused("scale", e))?;
        }
        check_frames(render.frames).map_err(|e| refused("frames", e))?;
        Ok(Self {
            render: RenderDefaults {
                size,
                scale: render.scale,
                trim: render.trim,
                frames: render.frames,
            },
            record: RecordDefaults::from(&config.record),
            steps: config.plays.steps,
            log: config.plays.log.map(LogMode::from),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct RecordDefaults {
    pub(crate) formats: Option<VideoFormats>,
    pub(crate) hold: Option<f32>,
    /// Where the keys and mouse panel goes; `None` for no panel.
    pub(crate) input: Option<Corner>,
    pub(crate) glide: Glide,
}

/// `[record]`'s own, for a catalog that sets none of it.
impl Default for RecordDefaults {
    fn default() -> Self {
        Self::from(&gallery_build::Record::default())
    }
}

impl From<&gallery_build::Record> for RecordDefaults {
    fn from(config: &gallery_build::Record) -> Self {
        let format = |format: &gallery_build::Format| match format {
            gallery_build::Format::Mp4 => VideoFormat::Mp4,
            gallery_build::Format::Apng => VideoFormat::Apng,
            gallery_build::Format::Gif => VideoFormat::Gif,
        };
        Self {
            formats: config
                .format
                .as_ref()
                .and_then(|formats| VideoFormats::of(formats.iter().map(format))),
            hold: config.hold,
            input: match config.input {
                gallery_build::Input::Off => None,
                gallery_build::Input::In(corner) => Some(corner.into()),
            },
            glide: config.glide,
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    /// What a catalog whose `gallery.toml` has `record` records with.
    fn read(record: &str) -> RecordDefaults {
        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let path = dir.join("gallery.toml");
        std::fs::write(
            &path,
            format!("scene_globs = [\"*.scene.rs\"]\n[record]\n{record}"),
        )
        .expect("written");
        let config = gallery_build::Config::read(&path).unwrap_or_else(|e| panic!("{e}"));
        RecordDefaults::from(&config.record)
    }

    #[test]
    fn a_catalogs_record_table_is_what_its_recordings_are_made_with() {
        assert_eq!(read(""), RecordDefaults::default());
        assert_eq!(
            read(r#"format = ["apng", "mp4"]"#).formats,
            VideoFormats::of([VideoFormat::Apng, VideoFormat::Mp4])
        );
        assert_eq!(read("hold = 0.25").hold, Some(0.25));
        assert_eq!(read("input = false").input, None);
        let glide = read("glide = \"linear\"\nglide_speed = 600").glide;
        assert_eq!(
            (glide.easing, glide.speed),
            (Some(gallery_build::Easing::Linear), 600.0)
        );
        for (name, corner) in [
            ("bottom-left", Corner::BottomLeft),
            ("bottom-right", Corner::BottomRight),
            ("top-left", Corner::TopLeft),
            ("top-right", Corner::TopRight),
        ] {
            assert_eq!(read(&format!("input = \"{name}\"")).input, Some(corner));
        }
    }

    /// The defaults of a catalog whose `gallery.toml` has `tables`.
    fn of(tables: toml::Table) -> Result<Defaults, String> {
        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let path = dir.join("gallery.toml");
        let mut config = toml::toml! { scene_globs = ["*.scene.rs"] };
        config.extend(tables);
        std::fs::write(&path, toml::to_string(&config).expect("TOML")).expect("written");
        Defaults::of(&gallery_build::Config::read(&path).unwrap_or_else(|e| panic!("{e}")))
    }

    #[test]
    fn a_catalogs_render_and_plays_tables_are_checked_as_a_shots_own_would_be() {
        let set = of(toml::toml! {
            [render]
            size = "640x360"
            scale = 2
            trim = false
            frames = 3

            [plays]
            steps = true
            log = "off"
        })
        .expect("defaults");
        assert_eq!(
            set.render,
            RenderDefaults {
                size: Some(egui::vec2(640.0, 360.0)),
                scale: Some(2.0),
                trim: Some(false),
                frames: Some(3),
            }
        );
        assert_eq!((set.steps, set.log), (Some(true), Some(LogMode::Off)));
        for (tables, says) in [
            (
                toml::toml! { [render] size = "wide" },
                "gallery.toml [render] size",
            ),
            (
                toml::toml! { [render] scale = 0 },
                "gallery.toml [render] scale",
            ),
            (
                toml::toml! { [render] frames = 0 },
                "gallery.toml [render] frames",
            ),
        ] {
            let error = of(tables).expect_err("refused");
            assert!(error.contains(says), "{error}");
        }
    }
}
