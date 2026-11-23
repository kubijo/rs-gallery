//! The input a play's recording can show on top of its frames:
//! the keys pressed, the mouse buttons held, and where the pointer is.
//!
//! It is drawn by egui in a harness of its own and laid over each recorded frame,
//! so the scene, the shot's PNG and its step screenshots never see it.

use std::collections::VecDeque;

use egui::{Event, PointerButton, Pos2};

use super::*;

/// The corner the keys and mouse panel sits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Corner {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

impl std::str::FromStr for Corner {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        match raw {
            "bottom-left" => Ok(Self::BottomLeft),
            "bottom-right" => Ok(Self::BottomRight),
            "top-left" => Ok(Self::TopLeft),
            "top-right" => Ok(Self::TopRight),
            _ => Err(format!(
                "`{raw}` is not a corner: `bottom-left`, `bottom-right`, `top-left` or `top-right`"
            )),
        }
    }
}

impl Corner {
    fn align(self) -> egui::Align2 {
        match self {
            Self::BottomLeft => egui::Align2::LEFT_BOTTOM,
            Self::BottomRight => egui::Align2::RIGHT_BOTTOM,
            Self::TopLeft => egui::Align2::LEFT_TOP,
            Self::TopRight => egui::Align2::RIGHT_TOP,
        }
    }

    fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }
}

/// Seconds a key stays listed after it is pressed.
const LISTED_FOR: f32 = 1.5;

/// A click is pressed and released a frame apart,
/// so a button stays lit this many seconds past its press to be seen at all.
const LIT_FOR: f32 = 0.3;

/// The most keys listed at once; an older one makes room.
const MOST_KEYS: usize = 6;

/// Everything the overlay shows at one frame.
#[derive(Clone, PartialEq, Debug, Default)]
pub(super) struct Shown {
    /// Oldest first.
    pub(super) keys: Vec<String>,
    pub(super) pointer: Option<Pos2>,
    /// Primary, secondary and middle.
    pub(super) buttons: [bool; 3],
    pub(super) wheel: bool,
}

impl Shown {
    fn panel(&self) -> bool {
        !self.keys.is_empty() || self.buttons.contains(&true) || self.wheel
    }

    fn is_empty(&self) -> bool {
        !self.panel() && self.pointer.is_none()
    }
}

/// A play's input so far, counted in the frames it drew.
pub(super) struct InputLog {
    listed_for: u64,
    lit_for: u64,
    frame: u64,
    keys: VecDeque<(u64, String)>,
    pointer: Option<Pos2>,
    held: [bool; 3],
    pressed_at: [Option<u64>; 3],
    wheeled_at: Option<u64>,
}

impl InputLog {
    pub(super) fn new(fps: u16) -> Self {
        let frames = |seconds: f32| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a second or two of frames"
            )]
            let frames = (seconds * f32::from(fps)).round() as u64;
            frames.max(1)
        };
        Self {
            listed_for: frames(LISTED_FOR),
            lit_for: frames(LIT_FOR),
            frame: 0,
            keys: VecDeque::new(),
            pointer: None,
            held: [false; 3],
            pressed_at: [None; 3],
            wheeled_at: None,
        }
    }

    /// Take the events one drawn frame was given.
    pub(super) fn frame(&mut self, events: &[Event]) {
        self.frame += 1;
        for event in events {
            match event {
                Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => self.list(
                    egui::KeyboardShortcut::new(*modifiers, *key)
                        .format(&egui::ModifierNames::NAMES, false),
                ),
                Event::Text(text) => self.list(format!("{text:?}")),
                Event::PointerMoved(at) => self.pointer = Some(*at),
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    self.pointer = Some(*pos);
                    if let Some(index) = button_index(*button) {
                        self.held[index] = *pressed;
                        if *pressed {
                            self.pressed_at[index] = Some(self.frame);
                        }
                    }
                }
                Event::MouseWheel { .. } => self.wheeled_at = Some(self.frame),
                _ => {}
            }
        }
        while self
            .keys
            .front()
            .is_some_and(|(at, _)| at + self.listed_for <= self.frame)
        {
            self.keys.pop_front();
        }
    }

    /// Let `frames` pass with no input, as a recording's pause does.
    pub(super) fn hold(&mut self, frames: u32) {
        self.frame += u64::from(frames);
    }

    fn list(&mut self, key: String) {
        if self.keys.len() == MOST_KEYS {
            self.keys.pop_front();
        }
        self.keys.push_back((self.frame, key));
    }

    fn lit(&self, at: Option<u64>) -> bool {
        at.is_some_and(|at| self.frame < at + self.lit_for)
    }

    pub(super) fn shown(&self) -> Shown {
        Shown {
            keys: self.keys.iter().map(|(_, key)| key.clone()).collect(),
            pointer: self.pointer,
            buttons: std::array::from_fn(|index| {
                self.held[index] || self.lit(self.pressed_at[index])
            }),
            wheel: self.lit(self.wheeled_at),
        }
    }
}

fn button_index(button: PointerButton) -> Option<usize> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Secondary => Some(1),
        PointerButton::Middle => Some(2),
        PointerButton::Extra1 | PointerButton::Extra2 => None,
    }
}

/// The overlay's own app: nothing but what [`Shown`] holds, on a clear canvas.
pub(super) struct OverlayApp {
    shown: Shown,
    corner: Corner,
}

const INK: egui::Color32 = egui::Color32::WHITE;
const BACKDROP: egui::Color32 = egui::Color32::from_black_alpha(170);
const LIT: egui::Color32 = egui::Color32::from_rgb(255, 196, 64);

impl eframe::App for OverlayApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.shown.panel() {
            egui::Area::new(egui::Id::new("gallery-input"))
                .anchor(self.corner.align(), corner_offset(self.corner))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(BACKDROP)
                        .corner_radius(8)
                        .inner_margin(10)
                        .show(ui, |ui| self.panel(ui));
                });
        }
        if let Some(at) = self.shown.pointer {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("gallery-pointer"),
            ));
            if self.shown.buttons.contains(&true) {
                painter.circle_filled(at, 10.0, LIT.gamma_multiply(0.6));
            }
            painter.circle_stroke(at, 10.0, egui::Stroke::new(2.0, egui::Color32::BLACK));
            painter.circle_stroke(at, 8.0, egui::Stroke::new(2.0, INK));
        }
    }
}

impl OverlayApp {
    /// The keys and the mouse, the newest key nearest the corner.
    fn panel(&self, ui: &mut egui::Ui) {
        // Left-aligned in every corner: aligned right,
        // a layout takes all the width offered.
        ui.vertical(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let key = |ui: &mut egui::Ui, text: &str| {
                ui.label(egui::RichText::new(text).size(22.0).strong().color(INK));
            };
            if self.corner.is_top() {
                self.mouse(ui);
                self.shown.keys.iter().rev().for_each(|text| key(ui, text));
            } else {
                self.shown.keys.iter().for_each(|text| key(ui, text));
                self.mouse(ui);
            }
        });
    }

    fn mouse(&self, ui: &mut egui::Ui) {
        let (body, _) = ui.allocate_exact_size(egui::vec2(28.0, 42.0), egui::Sense::hover());
        let painter = ui.painter();
        let split = body.top() + body.height() * 0.42;
        let wheel = 6.0;
        let [primary, secondary, middle] = self.shown.buttons;
        let fill = |rect: egui::Rect, lit: bool| {
            if lit {
                painter.rect_filled(rect.shrink(1.0), 3.0, LIT);
            }
        };
        fill(
            egui::Rect::from_min_max(body.min, egui::pos2(body.center().x - wheel / 2.0, split)),
            primary,
        );
        fill(
            egui::Rect::from_min_max(
                egui::pos2(body.center().x + wheel / 2.0, body.top()),
                egui::pos2(body.right(), split),
            ),
            secondary,
        );
        let wheel_rect = egui::Rect::from_center_size(
            egui::pos2(body.center().x, body.top() + body.height() * 0.22),
            egui::vec2(wheel - 2.0, body.height() * 0.24),
        );
        fill(wheel_rect, middle || self.shown.wheel);
        let line = egui::Stroke::new(1.5, INK);
        painter.rect_stroke(body, 12.0, line, egui::StrokeKind::Inside);
        painter.hline(body.x_range(), split, line);
        painter.vline(body.center().x, body.top()..=split, line);
        painter.rect_stroke(wheel_rect, 2.0, line, egui::StrokeKind::Outside);
    }
}

fn corner_offset(corner: Corner) -> egui::Vec2 {
    let margin = 12.0;
    let align = corner.align();
    egui::vec2(
        if align.x() == egui::Align::Max {
            -margin
        } else {
            margin
        },
        if corner.is_top() { margin } else { -margin },
    )
}

/// A recording's input overlay: the log, the harness that draws it,
/// and its last image, drawn again only when what it shows changes.
pub(super) struct InputOverlay {
    log: InputLog,
    harness: egui_kittest::Harness<'static, OverlayApp>,
    drawn: Option<(Shown, image::RgbaImage)>,
}

impl InputOverlay {
    /// An overlay for a canvas of `size` points at `scale`,
    /// drawn on the shot's own renderer.
    ///
    /// # Errors
    /// When the renderer cannot open a harness.
    pub(super) fn open(
        session: &Session,
        size: egui::Vec2,
        scale: f32,
        fps: u16,
        corner: Corner,
    ) -> Result<Self, Diagnostic> {
        let harness = open_apart(size, scale, session, |_| OverlayApp {
            shown: Shown::default(),
            corner,
        })?;
        Ok(Self {
            log: InputLog::new(fps),
            harness,
            drawn: None,
        })
    }

    /// Let the frames of a recording's pause pass,
    /// so a key pressed before it has gone once it is over.
    pub(super) fn hold(&mut self, frames: u32) {
        self.log.hold(frames);
    }

    /// Lay the input up to this frame, `events` included, over `frame`.
    ///
    /// # Errors
    /// When the overlay cannot be rendered.
    pub(super) fn lay_over(
        &mut self,
        events: &[Event],
        frame: &mut image::RgbaImage,
    ) -> Result<(), String> {
        self.log.frame(events);
        let shown = self.log.shown();
        if shown.is_empty() {
            return Ok(());
        }
        if self.drawn.as_ref().is_none_or(|(was, _)| *was != shown) {
            self.harness.state_mut().shown = shown.clone();
            // An area only learns its size on the first frame it is drawn,
            // so a panel anchored to a bottom or right edge lands there on the second.
            self.harness.step();
            self.harness.step();
            let harness = &mut self.harness;
            let image = std::panic::catch_unwind(AssertUnwindSafe(|| harness.render())).map_err(
                |panic| format!("render the input overlay: {}", panic_message(&panic)),
            )??;
            self.drawn = Some((shown, image));
        }
        let (_, overlay) = self.drawn.as_ref().expect("BUG: drawn just above");
        if overlay.dimensions() != frame.dimensions() {
            return Err(format!(
                "the input overlay came out {:?} for a frame of {:?}",
                overlay.dimensions(),
                frame.dimensions()
            ));
        }
        composite(frame, overlay);
        Ok(())
    }
}

/// `over` laid on `under`; `over` carries premultiplied alpha, as egui renders it.
pub(super) fn composite(under: &mut image::RgbaImage, over: &image::RgbaImage) {
    for (below, above) in under.pixels_mut().zip(over.pixels()) {
        let alpha = u16::from(above[3]);
        if alpha == 0 {
            continue;
        }
        for channel in 0..3 {
            let blended =
                u16::from(above[channel]) + u16::from(below[channel]) * (255 - alpha) / 255;
            below[channel] = u8::try_from(blended.min(255)).expect("BUG: clamped to a byte");
        }
        below[3] = below[3].max(above[3]);
    }
}
