//! Scenes read from a dylib the launcher builds, and — under `--hot` — rebuilds and swaps live.

use std::time::Duration;

use hot_lib_reloader::LibReloader;

use crate::{Manifest, SceneSource};

/// A [`SceneSource`] reading scenes from a reloaded dylib: the dylib exports
/// `__gallery_manifest() -> Manifest`, hot-swapped as it is rebuilt. The dylib directory comes from
/// the running executable, so it follows any `CARGO_TARGET_DIR`. Both sides must share one
/// gallery/egui version — a single workspace lock guarantees it.
pub struct HotDylib {
    reloader: LibReloader,
    watching: bool,
}

impl HotDylib {
    /// Load `lib<lib_name>.<dylib-ext>` from the current executable's directory — the same
    /// `<target>/<profile>/` cargo drops both the host binary and the dylib into.
    ///
    /// `watching` says whether a watcher is rebuilding that dylib (`--hot`). Only then is there
    /// anything to poll for.
    ///
    /// # Errors
    /// If the executable path can't be read, or the dylib can't be loaded from that directory.
    pub fn new(lib_name: &str, watching: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or("current executable has no parent directory")?;
        let dir = camino::Utf8Path::from_path(dir).ok_or("executable path is not UTF-8")?;
        let reloader = LibReloader::new(dir, lib_name, Some(Duration::from_millis(200)), None)?;
        Ok(Self { reloader, watching })
    }
}

impl SceneSource for HotDylib {
    fn before_frame(&mut self, ctx: &egui::Context) {
        // Polling unconditionally kept the shell repainting 5×/s forever, so it never came to rest
        // and a frame-cost reading had nothing at rest to measure.
        if !self.watching {
            return;
        }
        // Swap in a rebuilt dylib, then keep polling so edits show without user input.
        let _ = self.reloader.update();
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn manifest(&mut self) -> Manifest {
        // SAFETY: `__gallery_manifest` is exported by the scenes dylib built against the same gallery
        // (one workspace lock), so `Manifest`/`SceneEntry` layouts match. Its `&'static str`s point
        // into the loaded library and are used only this frame (before the next `update()`).
        let entry = unsafe {
            self.reloader
                .get_symbol::<fn() -> Manifest>(b"__gallery_manifest\0")
        };
        match entry {
            Ok(manifest) => manifest(),
            Err(_) => Manifest {
                scenes: Vec::new(),
                groups: Vec::new(),
            },
        }
    }
}
