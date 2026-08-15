//! Scenes read from a dylib the launcher builds, and — under `--hot` — rebuilds and swaps live.
//!
//! The opening is gallery's own for one reason: a library a scene has drawn from can never be
//! closed. See [`HotDylib`].

use std::{
    fs,
    mem::ManuallyDrop,
    time::{Duration, Instant, SystemTime},
};

use camino::{Utf8Path, Utf8PathBuf};
use libloading::Library;

use crate::{Manifest, SceneSource, watch::HotStatus};

/// What the platform calls a dynamic library.
#[cfg(target_os = "windows")]
const PREFIX: &str = "";
#[cfg(not(target_os = "windows"))]
const PREFIX: &str = "lib";
#[cfg(target_os = "windows")]
const EXTENSION: &str = "dll";
#[cfg(target_os = "macos")]
const EXTENSION: &str = "dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const EXTENSION: &str = "so";

/// How often a watching shell looks for a rebuilt dylib.
const POLL: Duration = Duration::from_millis(200);

/// How often it draws while the cycle is moving, an elapsed count being unreadable at [`POLL`].
const LIVE: Duration = Duration::from_millis(100);

/// How long the dylib has to stand still before it is opened: cargo copies the artifact into place
/// where it cannot hard-link it, and a copy is not atomic.
const SETTLE: Duration = Duration::from_millis(200);

/// A [`SceneSource`] reading scenes from the dylib the launcher builds: it exports
/// `__gallery_manifest() -> Manifest`, and a rebuild is opened over it. The dylib directory comes
/// from the running executable, so it follows any `CARGO_TARGET_DIR`. Both sides must share one
/// gallery/egui version — a single workspace lock guarantees it.
pub struct HotDylib {
    /// Every library opened, the last being the scenes on screen.
    ///
    /// None is ever closed, hence `ManuallyDrop`.
    ///
    /// A widget's `ui.data_mut(..)` state is a `Box<dyn Any>` whose vtable lives in the library
    /// that boxed it, and egui's `Memory` holds those boxes as long as its `Context` lives.
    /// Closing one a scene had drawn from left them dangling, and the virtual `type_id` call
    /// inside `downcast_ref` then jumped into unmapped memory on the first frame after a swap
    /// — a `Grid` in `SceneCtx::matrix_with` was enough. The destructors did the same on close.
    /// Keeping the mappings costs address space.
    loaded: ManuallyDrop<Vec<Library>>,
    /// Where cargo writes the dylib: the file a rebuild is noticed by.
    dylib: Utf8PathBuf,
    /// The copy the current library was opened from, kept for a backtrace to name.
    copy: Option<Utf8PathBuf>,
    /// How many have been opened, which is what keeps each copy off a mapped path.
    opens: usize,
    /// What the dylib looked like when it was opened.
    from: Option<Written>,
    /// A writing noticed but not yet opened.
    seen: Option<Seen>,
    /// The cycle a swap is reported into, `Some` exactly when something is rebuilding this dylib.
    /// The launcher puts the one its watcher and window share here, over the one made below — what
    /// a host driving [`run`](crate::run) itself is left with.
    pub(crate) hot: Option<HotStatus>,
}

/// Enough of a file to notice that it was written again.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Written {
    at: SystemTime,
    len: u64,
}

/// A writing, and when it was noticed.
#[derive(Clone, Copy, Debug)]
struct Seen {
    written: Written,
    at: Instant,
}

impl HotDylib {
    /// Load `lib<lib_name>.<dylib-ext>` from the current executable's directory — the same
    /// `<target>/<profile>/` cargo drops both the host binary and the dylib into.
    ///
    /// `watching` says whether a watcher is rebuilding that dylib (`--hot`). Only then is there
    /// anything to poll for.
    ///
    /// # Errors
    /// If the executable path can't be read, or the dylib can't be opened from that directory.
    pub fn new(lib_name: &str, watching: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or("current executable has no parent directory")?;
        let dir = camino::Utf8Path::from_path(dir).ok_or("executable path is not UTF-8")?;
        let mut scenes = Self {
            loaded: ManuallyDrop::new(Vec::new()),
            dylib: dir.join(format!("{PREFIX}{lib_name}.{EXTENSION}")),
            copy: None,
            opens: 0,
            from: None,
            seen: None,
            hot: watching.then(HotStatus::new),
        };
        // What the launcher just built. Nothing there is not a failure: a run waiting on its first
        // build has an empty manifest until one lands.
        if let Some(written) = written(&scenes.dylib) {
            scenes.open(written)?;
        }
        Ok(scenes)
    }

    /// Copy the dylib aside and open it, keeping open whatever it replaces. The copy is what gets
    /// opened, so the next rebuild writes the original rather than a file this process has mapped.
    fn open(&mut self, written: Written) -> Result<(), String> {
        let stem = self.dylib.file_stem().unwrap_or("scenes");
        let copy = self
            .dylib
            .with_file_name(format!("{stem}-hot-{}.{EXTENSION}", self.opens));
        fs::copy(&self.dylib, &copy).map_err(|e| format!("copy `{}`: {e}", self.dylib))?;
        codesign(&copy);

        // SAFETY: the file is the scenes dylib gallery built, and opening one runs its initialisers
        // — the `inventory` registrations behind `__gallery_manifest`.
        let library = unsafe { Library::new(copy.as_std_path()) }
            .map_err(|e| format!("open `{copy}`: {e}"))?;

        self.loaded.push(library);
        self.opens += 1;
        self.from = Some(written);
        if let Some(replaced) = self.copy.replace(copy) {
            discard(&replaced);
        }
        Ok(())
    }

    /// Open a rebuilt dylib once it has stood still, and say whether one was opened.
    fn swap_if_rebuilt(&mut self) -> bool {
        let Some(written) = written(&self.dylib) else {
            return false;
        };
        if self.from == Some(written) {
            return false;
        }
        if !settled(self.seen, written) {
            if self.seen.map(|seen| seen.written) != Some(written) {
                self.seen = Some(Seen {
                    written,
                    at: Instant::now(),
                });
            }
            return false;
        }

        self.seen = None;
        if let Err(e) = self.open(written) {
            eprintln!("gallery: the rebuilt scenes could not be opened: {e}");
            // Taken as opened all the same, or every frame would retry it and say so again.
            self.from = Some(written);
            return false;
        }
        true
    }
}

/// Whether a dylib that has changed since it was opened has stood still long enough to open.
fn settled(seen: Option<Seen>, written: Written) -> bool {
    seen.is_some_and(|seen| seen.written == written && seen.at.elapsed() >= SETTLE)
}

/// What the file looks like now, or `None` where there is no file to look at.
fn written(path: &Utf8Path) -> Option<Written> {
    let file = fs::metadata(path).ok()?;
    Some(Written {
        at: file.modified().ok()?,
        len: file.len(),
    })
}

/// Drop a copy that has been replaced. A unix mapping outlives its file, so only the predecessor
/// goes and the one in use stays for a backtrace to name.
/// Windows holds a loaded file open, where they stay.
fn discard(copy: &Utf8Path) {
    #[cfg(unix)]
    let _ = fs::remove_file(copy);
    #[cfg(not(unix))]
    let _ = copy;
}

/// Sign the copy ad-hoc, which is what lets macOS open a library that has moved
/// (<https://github.com/rksm/hot-lib-reloader-rs/issues/15>).
#[cfg(target_os = "macos")]
fn codesign(copy: &Utf8Path) {
    let signed = std::process::Command::new("codesign")
        .args(["--sign", "-", "--force", copy.as_str()])
        .status();
    if !signed.is_ok_and(|status| status.success()) {
        eprintln!("gallery: `codesign` did not run — macOS will refuse the rebuilt scenes");
    }
}

#[cfg(not(target_os = "macos"))]
fn codesign(_copy: &Utf8Path) {}

impl SceneSource for HotDylib {
    fn before_frame(&mut self, ctx: &egui::Context) {
        // Polling unconditionally kept the shell repainting 5×/s forever, so it never came to rest
        // and a frame-cost reading had nothing at rest to measure.
        let Some(hot) = self.hot.clone() else { return };
        // The watcher's thread has no other way to reach the window.
        hot.wake_with(ctx);
        // Swap in a rebuilt dylib, then keep polling so edits show without user input.
        if self.swap_if_rebuilt() {
            hot.swapped();
        }
        hot.settle();
        ctx.request_repaint_after(if hot.is_moving() { LIVE } else { POLL });
    }

    fn manifest(&mut self) -> Manifest {
        let nothing = || Manifest {
            scenes: Vec::new(),
            groups: Vec::new(),
        };
        let Some(library) = self.loaded.last() else {
            return nothing();
        };
        // SAFETY: `__gallery_manifest` is exported by the scenes dylib, built against the same
        // gallery (one workspace lock), so `Manifest`/`SceneEntry` layouts match.
        // Its `&'static str`s point into a library never closed, so they last as long as the run.
        let entry = unsafe { library.get::<fn() -> Manifest>(b"__gallery_manifest\0") };
        match entry {
            Ok(manifest) => manifest(),
            Err(_) => nothing(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written_at(len: u64) -> Written {
        Written {
            at: SystemTime::UNIX_EPOCH,
            len,
        }
    }

    #[test]
    fn a_dylib_is_opened_only_once_it_has_stopped_being_written() {
        let written = written_at(100);
        let seen = |written, ago| {
            Some(Seen {
                written,
                at: Instant::now() - ago,
            })
        };

        assert!(
            !settled(None, written),
            "a change noticed this instant has not stood still yet"
        );
        assert!(
            !settled(seen(written, Duration::ZERO), written),
            "nor has one still inside the settling time"
        );
        assert!(
            !settled(seen(written_at(60), SETTLE), written),
            "a file that is still growing starts the wait over"
        );
        assert!(
            settled(seen(written, SETTLE), written),
            "the same file, still, for long enough: cargo has finished writing it"
        );
    }

    /// The name is the platform's, and each copy its own file
    /// — opening one over a mapping is what the copies exist to avoid.
    #[test]
    fn each_open_copies_the_dylib_to_a_name_of_its_own() {
        let dylib = Utf8PathBuf::from(format!("/t/{PREFIX}app_gallery.{EXTENSION}"));
        let stem = dylib.file_stem().expect("a stem");

        let copies: Vec<Utf8PathBuf> = (0..2)
            .map(|open| dylib.with_file_name(format!("{stem}-hot-{open}.{EXTENSION}")))
            .collect();

        assert_ne!(copies[0], copies[1]);
        assert!(copies.iter().all(|copy| copy != &dylib), "{copies:?}");
        assert!(copies[0].as_str().ends_with(EXTENSION), "{:?}", copies[0]);
    }
}
