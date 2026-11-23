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
}

impl crate::play::PlayDriver for HeadlessDriver<'_> {
    fn query(&self, by: &crate::By) -> Vec<crate::Found> {
        use kittest::NodeT as _;
        crate::play::query_tree(
            self.harness.root().accesskit_node(),
            self.harness.ctx.pixels_per_point(),
            by,
        )
    }

    fn labels(&self) -> Vec<String> {
        use kittest::NodeT as _;
        crate::play::tree_labels(self.harness.root().accesskit_node())
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
        self.harness.step();
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
        let Some(dir) = &self.trace_dir else {
            return Ok(None);
        };
        let drawn = self.harness.state().wanted;
        // Caught here, by the `std` that raised it: unwinding into a scenes dylib's frames,
        // a panic would meet the dylib's own `std`, which aborts on it.
        let image = std::panic::catch_unwind(AssertUnwindSafe(|| self.harness.render())).map_err(
            |panic| format!("render the step's screenshot: {}", panic_message(&panic)),
        )??;
        let image = if self.trim {
            trim(image, drawn, self.scale)
        } else {
            image
        };
        std::fs::create_dir_all(dir).map_err(|e| format!("create `{dir}`: {e}"))?;
        let path = dir.join(format!("{:02}-{}.png", index + 1, step_file_stem(label)));
        write_png(&image, &path)?;
        Ok(Some(landed(&path)))
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

/// A step label as a file name: lower-case words, single dashes, cut short,
/// since a gesture's label spells out its locator.
fn step_file_stem(label: &str) -> String {
    const MOST: usize = 48;
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut cut: String = slug.chars().take(MOST).collect();
    while cut.ends_with('-') {
        cut.pop();
    }
    if cut.is_empty() {
        cut.push_str("step");
    }
    cut
}
