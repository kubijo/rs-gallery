//! How a catalog records unless a shot or a flag says otherwise: its `gallery.toml`'s `[record]`.
//! Handed to what records, so a test can record as any catalog would.

use super::{Corner, VideoFormat, VideoFormats};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct RecordDefaults {
    pub(crate) formats: Option<VideoFormats>,
    pub(crate) hold: Option<f32>,
    /// Where the keys and mouse panel goes; `None` for no panel.
    pub(crate) input: Option<Corner>,
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
        for (name, corner) in [
            ("bottom-left", Corner::BottomLeft),
            ("bottom-right", Corner::BottomRight),
            ("top-left", Corner::TopLeft),
            ("top-right", Corner::TopRight),
        ] {
            assert_eq!(read(&format!("input = \"{name}\"")).input, Some(corner));
        }
    }
}
