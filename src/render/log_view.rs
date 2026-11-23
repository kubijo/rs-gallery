//! A play's log as a capture runs it: redrawn in place on a terminal,
//! printed once the play is over everywhere else.
//!
//! Both show the same lines, so a log read
//! in a terminal and one read from a file agree.

use std::io::{IsTerminal as _, Write};

use super::*;

/// How a run shows its plays as they go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LogMode {
    /// The running play's tree, redrawn in place.
    Live,
    /// Each play's tree once it is over, never redrawn.
    Linear,
    Off,
}

impl std::str::FromStr for LogMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        match raw {
            "live" => Ok(Self::Live),
            "linear" => Ok(Self::Linear),
            "off" => Ok(Self::Off),
            _ => Err(format!(
                "`{raw}` is not a log mode: `live`, `linear` or `off`"
            )),
        }
    }
}

impl LogMode {
    /// `asked` when given, else `GALLERY_LOG`, else live
    /// only where a terminal can redraw: stderr a terminal,
    /// `TERM` not `dumb`, and no `CI` set.
    ///
    /// # Errors
    /// When `GALLERY_LOG` names no mode.
    pub(crate) fn detect(asked: Option<Self>) -> Result<Self, String> {
        if let Some(asked) = asked {
            return Ok(asked);
        }
        if let Ok(raw) = std::env::var("GALLERY_LOG") {
            return raw.parse().map_err(|e| format!("GALLERY_LOG: {e}"));
        }
        Ok(Self::auto(
            std::io::stderr().is_terminal(),
            std::env::var_os("CI").is_some(),
            std::env::var("TERM").ok().as_deref(),
        ))
    }

    pub(super) fn auto(terminal: bool, ci: bool, term: Option<&str>) -> Self {
        if terminal && !ci && term != Some("dumb") {
            Self::Live
        } else {
            Self::Linear
        }
    }
}

/// `GALLERY_LOG_PACE`, in milliseconds. A value that is no number
/// is warned about rather than failing the run it would only have slowed.
pub(super) fn pace(raw: Option<&str>) -> std::time::Duration {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return std::time::Duration::ZERO;
    };
    raw.parse()
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|_| {
            eprintln!("GALLERY_LOG_PACE: `{raw}` is not a number of milliseconds; not pacing");
            std::time::Duration::ZERO
        })
}

/// Where a run's plays are shown as they go.
pub(crate) struct LogView {
    mode: LogMode,
    out: Box<dyn Write>,
    colour: bool,
    /// The most rows the live area takes, and how wide a row may be.
    rows: usize,
    columns: usize,
    /// How long each live redraw stays before the play goes on,
    /// slowing a run down enough to watch or record it.
    pace: std::time::Duration,
    playing: Option<Playing>,
}

struct Playing {
    shot: String,
    play: String,
    /// Kept whole once over, rather
    /// than folded to one line when it passed.
    expand: bool,
    log: Vec<Entry>,
    /// Rows the live area holds now,
    /// which the next redraw goes back over.
    drawn: usize,
}

impl LogView {
    /// A view on stderr.
    pub(crate) fn new(mode: LogMode) -> Self {
        let stderr = std::io::stderr();
        let colour = anstream::AutoStream::choice(&stderr) != anstream::ColorChoice::Never;
        let (columns, rows) = terminal_size::terminal_size_of(&stderr).map_or((80, 24), |size| {
            (usize::from((size.0).0), usize::from((size.1).0))
        });
        Self {
            mode,
            out: Box::new(stderr),
            colour,
            // A row is left for the prompt,
            // which the area would otherwise push off the top.
            rows: rows.saturating_sub(1).max(3),
            columns: columns.max(20),
            pace: pace(std::env::var("GALLERY_LOG_PACE").ok().as_deref()),
            playing: None,
        }
    }

    pub(crate) fn off() -> Self {
        Self::new(LogMode::Off)
    }

    #[cfg(test)]
    pub(super) fn into(mode: LogMode, out: Box<dyn Write>, rows: usize, columns: usize) -> Self {
        Self {
            mode,
            out,
            colour: false,
            rows,
            columns,
            pace: std::time::Duration::ZERO,
            playing: None,
        }
    }

    /// A play starts on `shot`.
    pub(super) fn begin(&mut self, shot: &str, play: &str, expand: bool) {
        if self.mode == LogMode::Off {
            return;
        }
        self.playing = Some(Playing {
            shot: shot.to_owned(),
            play: play.to_owned(),
            expand,
            log: Vec::new(),
            drawn: 0,
        });
        self.redraw();
    }

    /// The play's entry `index` began, or ended as `entry` now says.
    pub(super) fn logged(&mut self, index: usize, entry: &Entry) {
        let Some(playing) = &mut self.playing else {
            return;
        };
        match playing.log.get_mut(index) {
            Some(known) => *known = entry.clone(),
            None => playing.log.push(entry.clone()),
        }
        self.redraw();
    }

    /// The play is over: its tree stays when
    /// it failed or was asked for, else one line does.
    pub(super) fn end(&mut self, failed: bool, frames: u32) {
        let Some(playing) = self.playing.take() else {
            return;
        };
        let text = if failed || playing.expand {
            step_list(&playing.shot, &playing.play, &playing.log, failed)
        } else {
            let aside = Style::new().dimmed();
            format!(
                "{} {} {}",
                paint(Style::new().fg_color(Some(AnsiColor::Green.into())), "✓"),
                paint(Style::new().bold(), &playing.shot),
                paint(
                    aside,
                    &format!(
                        "· {}  {frames} frame{}",
                        playing.play,
                        if frames == 1 { "" } else { "s" }
                    )
                ),
            )
        };
        let rewind = match self.mode {
            LogMode::Live => playing.drawn,
            LogMode::Linear | LogMode::Off => 0,
        };
        // Over, it is printed whole: it is the only copy left once the area moves on.
        self.write(rewind, text.lines());
    }

    /// The live area again, showing the running play's tree as it stands.
    fn redraw(&mut self) {
        if self.mode != LogMode::Live {
            return;
        }
        let Some(playing) = &self.playing else {
            return;
        };
        let text = step_list(&playing.shot, &playing.play, &playing.log, false);

        // A play only just begun has done nothing yet,
        // which is not the same as having no steps.
        let lines: Vec<&str> = text
            .lines()
            .take(if playing.log.is_empty() {
                1
            } else {
                usize::MAX
            })
            .collect();

        // The header, then the newest lines that fit: an area taller than the terminal
        // cannot be gone back over, and would leave its top behind on every redraw.
        let shown: Vec<&str> = if lines.len() > self.rows {
            std::iter::once(lines[0])
                .chain(lines[lines.len() - (self.rows - 1)..].iter().copied())
                .collect()
        } else {
            lines
        };
        let rewind = playing.drawn;
        let count = shown.len();
        let fitted: Vec<String> = shown.into_iter().map(|line| self.fit(line)).collect();
        self.write(rewind, fitted.iter().map(String::as_str));
        if let Some(playing) = &mut self.playing {
            playing.drawn = count;
        }
        if !self.pace.is_zero() {
            std::thread::sleep(self.pace);
        }
    }

    /// A live row cut to the terminal's width: one that wrapped would take two rows,
    /// and the next redraw would go back over one too few.
    fn fit(&self, line: &str) -> String {
        console::truncate_str(line, self.columns, "…").into_owned()
    }

    /// Go back over the `rewind` rows last drawn,
    /// clear them, and write `lines` in their place.
    fn write<'a>(&mut self, rewind: usize, lines: impl Iterator<Item = &'a str>) {
        let mut text = String::new();
        if rewind > 0 {
            use crossterm::{
                Command as _,
                cursor::MoveToPreviousLine,
                terminal::{Clear, ClearType},
            };
            let rows = u16::try_from(rewind).unwrap_or(u16::MAX);
            _ = MoveToPreviousLine(rows).write_ansi(&mut text);
            _ = Clear(ClearType::FromCursorDown).write_ansi(&mut text);
        }
        for line in lines {
            if self.colour {
                text.push_str(line);
            } else {
                text.push_str(&style::plain(line));
            }
            text.push('\n');
        }
        // The log is a courtesy to whoever watches;
        // a closed stderr is no reason to fail a run.
        _ = self.out.write_all(text.as_bytes());
        _ = self.out.flush();
    }
}
