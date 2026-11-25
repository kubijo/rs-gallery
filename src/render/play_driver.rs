//! [`HeadlessDriver`], and the step screenshots a play leaves beside its shot.

use super::*;

/// A play's way into a capture harness.
/// Each frame re-applies the shot's globals and knobs, as [`settle`] does,
/// so the recipe outranks the scene and the play outranks both.
pub(super) struct HeadlessDriver<'h> {
    pub(super) harness: &'h mut egui_kittest::Harness<'static, Canvas>,
    pub(super) globals: &'h [KnobOverride],
    /// The shot's knobs, with the play's own on top.
    pub(super) ledger: Ledger,
    pub(super) scale: f32,
    pub(super) trim: bool,
    /// Step screenshots go here: [`steps_dir`] of the shot's PNG.
    pub(super) trace_dir: Option<Utf8PathBuf>,
    /// The key whose value failed the last frame.
    pub(super) bad_knob: Option<String>,
    /// Where the play's log is shown as it goes.
    pub(super) view: &'h mut LogView,
    pub(super) session: &'h Session,
    /// The canvas now, in points.
    pub(super) size: &'h mut egui::Vec2,
    /// `None` in a capture, which compares no snapshots.
    pub(super) looks: Option<Looks<'h>>,
}

impl HeadlessDriver<'_> {
    fn outgrown_by(&self, drawn: egui::Vec2) -> bool {
        self.size.max(drawn.ceil()) != *self.size
    }

    /// Grow the canvas to hold `drawn`.
    /// The next frame is the first drawn at that size.
    fn grow(&mut self, drawn: egui::Vec2) -> Result<(), String> {
        let fitting = self.size.max(drawn.ceil());
        self.session
            .validate_target(fitting, self.scale)
            .map_err(|e| e.summary())?;
        self.harness.set_size(fitting);
        *self.size = fitting;
        Ok(())
    }

    /// Compare what `ask` is of, as it is now.
    fn look(&mut self, looks: &mut Looks<'_>, ask: &Ask<'_>) -> Result<Look, String> {
        use kittest::NodeT as _;
        if ask.first {
            looks.begin(ask)?;
        }
        let seen = match ask.of {
            Of::Screenshot { .. } => {
                let drawn = drawn(self.harness);
                if self.outgrown_by(drawn) {
                    if looks.measured_twice(drawn) {
                        self.grow(drawn)?;
                    }
                    return Ok(looks.outgrown());
                }
                let image = rendered("render the canvas", self.harness)?;
                Seen::Image(trim(image, drawn, self.scale))
            }
            Of::Tree { by } => {
                let root = self.harness.root().accesskit_node();
                match read_tree(|| crate::play::tree::json(root, by)) {
                    Ok(Ok(json)) => Seen::Text(json),
                    Ok(Err(why)) => return Ok(looks.unmatched(why)),
                    Err(why) => return Ok(looks.unmatched(Unmatched::Unread(why))),
                }
            }
        };
        looks.judge(ask, seen)
    }
}

/// `body`'s panic caught here, by the `std` that raised it: unwinding into
/// a scenes dylib's frames, it would meet the dylib's own `std`, which aborts on it.
///
/// The other way round, so does a panic the dylib raises in here,
/// as a scene's paint callback can: [`crate::play::IN_SCENES_DYLIB`].
pub(super) fn guarded<T>(
    what: &str,
    body: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(body))
        .map_err(|panic| format!("{what}: {}", panic_message(&panic)))?
}

/// The canvas as last drawn, rendered [`guarded`]. Meanwhile a play's panics are not kept
/// off the terminal: the hook alone can tell of one a paint callback in the dylib raises.
pub(super) fn rendered(
    what: &str,
    harness: &mut egui_kittest::Harness<'static, Canvas>,
) -> Result<image::RgbaImage, String> {
    let quieten = harness.state().quieten;
    let was = quieten.map(|quieten| quieten(false));
    let image = guarded(what, || harness.render());
    if let (Some(quieten), Some(was)) = (quieten, was) {
        quieten(was);
    }
    image
}

/// [`guarded`], of what reads labels, quietly, as a wait may ask for one every frame:
/// accesskit_consumer 0.35 panics on a `labelled_by` naming a node not in the tree.
fn read_tree<T>(body: impl FnOnce() -> T) -> Result<T, String> {
    crate::play::caught(body).map_err(|panic| {
        format!(
            "the accessibility tree can't be read; a widget may be labelled by one \
             not drawn this frame: {}",
            panic.reason()
        )
    })
}

impl crate::play::PlayDriver for HeadlessDriver<'_> {
    fn quietened_by(&mut self, quieten: crate::play::Quieten) {
        self.harness.state_mut().quieten = Some(quieten);
    }

    fn query(&self, by: &crate::By) -> Result<Vec<crate::Found>, String> {
        use kittest::NodeT as _;
        read_tree(|| {
            crate::play::query_tree(
                self.harness.root().accesskit_node(),
                self.harness.ctx.pixels_per_point(),
                by,
            )
        })
    }

    fn labels(&self) -> Vec<String> {
        use kittest::NodeT as _;
        read_tree(|| crate::play::tree_labels(self.harness.root().accesskit_node()))
            .unwrap_or_default()
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Result<(), String> {
        self.bad_knob = None;
        let mut pending: Vec<&KnobOverride> = self.globals.iter().collect();
        apply_globals(self.harness.state_mut(), self.globals, &mut pending)
            .map_err(|e| e.summary())?;
        // A knob the scene declares only some of the time is no error on the frames it doesn't;
        // the play answers for a key that never landed, once it is over.
        if let Err(failed) = self.ledger.apply(self.harness.state_mut()) {
            self.bad_knob = Some(failed.key);
            return Err(failed.error.summary());
        }
        self.harness.input_mut().events.extend(events);
        // With nothing queued on the harness, `step` draws exactly one frame.
        step(self.harness)?;
        if let Some(looks) = &mut self.looks {
            looks.drew(self.harness.ctx.has_requested_repaint());
        }
        match self.harness.state_mut().global_write_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take_actions(&mut self) -> Vec<String> {
        self.harness
            .state_mut()
            .actions
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    fn set_knob(&mut self, key: &str, value: String) {
        self.ledger.set(KnobOverride {
            key: key.to_owned(),
            value,
        });
    }

    fn unlanded(&self) -> Vec<crate::play::Unlanded> {
        let bound = &self.harness.state().bound;
        self.ledger
            .unlanded()
            .map(|over| {
                let key = &over.key;
                let why = self.ledger.reason(key).map(str::to_owned).or_else(|| {
                    bound.label(key).map(|label| {
                        format!("knob `{label}`, which `{key}` matched before, is not declared now, so the value never landed")
                    })
                });
                crate::play::Unlanded {
                    key: key.clone(),
                    why,
                }
            })
            .collect()
    }

    fn bad_knob(&self) -> Option<String> {
        self.bad_knob.clone()
    }

    fn trace(&mut self, index: usize, label: &str) -> Result<Option<Utf8PathBuf>, String> {
        let Some(dir) = self.trace_dir.clone() else {
            return Ok(None);
        };
        let drawn = drawn(self.harness);
        let mut image = rendered("render the canvas", self.harness)?;
        if self.trim {
            image = trim(image, drawn, self.scale);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("create `{dir}`: {e}"))?;
        let path = dir.join(format!("{:02}-{}.png", index + 1, step_file_stem(label)));
        write_png(&image, &path)?;
        Ok(Some(landed(&path)))
    }

    fn end_step(&mut self) {
        hold(self.harness);
    }

    fn logged(&mut self, index: usize, entry: &Entry) {
        self.view.logged(index, entry);
    }

    fn compare(&mut self, ask: &Ask<'_>) -> Result<Look, String> {
        let Some(mut looks) = self.looks.take() else {
            return Ok(Look {
                verdict: Verdict::NotCompared,
                done: true,
            });
        };
        let look = guarded("compare the snapshot", || self.look(&mut looks, ask));
        self.looks = Some(looks);
        look
    }

    fn keep(&mut self) -> Result<crate::play::log::Kept, String> {
        guarded("keep the snapshot", || match &mut self.looks {
            Some(looks) => looks.keep(),
            None => Ok(crate::play::log::Kept::default()),
        })
    }
}

/// Where a shot's step screenshots go: `<stem>.steps/` beside its PNG.
/// The suffix keeps it apart from a folder of the shot's own name, and from the PNG itself
/// when its path has no extension.
pub(super) fn steps_dir(out: &Utf8Path) -> Utf8PathBuf {
    out.with_file_name(format!("{}.steps", out.file_stem().unwrap_or("shot")))
}

/// Remove the step screenshots in `dir`, only the files [`is_trace_file`] accepts,
/// then `dir` itself if that left it empty.
pub(super) fn clear_trace(dir: &Utf8Path) -> Result<(), Diagnostic> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(is_trace_file) {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("remove a trace screenshot in `{dir}`: {e}"))?;
        }
    }
    // Removes the directory only if nothing else is in it.
    _ = std::fs::remove_dir(dir);
    Ok(())
}

fn is_trace_file(name: &str) -> bool {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    digits >= 2 && name[digits..].starts_with('-') && name.ends_with(".png")
}

/// `label` in lower-case words and single dashes, empty when it has no letter or digit.
pub(super) fn dashed(label: &str) -> String {
    let mut words = String::new();
    for c in label.chars() {
        if c.is_alphanumeric() {
            words.extend(c.to_lowercase());
        } else if !words.is_empty() && !words.ends_with('-') {
            words.push('-');
        }
    }
    if words.ends_with('-') {
        words.pop();
    }
    words
}

/// A step label as a file name, [`dashed`] and cut short,
/// since a gesture's label spells out its locator.
pub(super) fn step_file_stem(label: &str) -> String {
    const MOST: usize = 48;
    let mut cut: String = dashed(label).chars().take(MOST).collect();
    if cut.ends_with('-') {
        cut.pop();
    }
    if cut.is_empty() {
        cut.push_str("step");
    }
    cut
}
