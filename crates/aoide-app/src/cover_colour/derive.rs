//! What a cover's main colour becomes in the app, for one mode: the wash
//! behind a page's header, the tint in the window's shell, and the accent,
//! with the label a filled button takes on it. Each is found by moving the
//! cover's colour, in its own hue, until everything drawn with or on it
//! reads: text at 4.5:1 or better, so the parts of the interface drawn in
//! the accent (3:1 is their floor) pass too.

use iced::Color;
use iced::theme::palette::deviate;

use super::maths::{Hsv, contrast, over};

/// How opaque the wash is at the top of the header, where it is strongest,
/// and halfway down; it is clear by the header's foot.
pub const WASH_TOP: f32 = 0.8;
pub const WASH_MIDDLE: f32 = 0.3;
/// How much of the tint goes into the sidebar's, the side panel's and the
/// player bar's grounds.
pub const TINT_DARK: f32 = 0.12;
pub const TINT_LIGHT: f32 = 0.06;

/// Text's floor, with a hair over it so a colour rounded to bytes on screen
/// stays above.
const TEXT: f32 = 4.5 + 0.05;
/// The wash's strengths at which its text is checked: the top, the middle,
/// and between.
const WASH_STEPS: [f32; 3] = [WASH_TOP, (WASH_TOP + WASH_MIDDLE) / 2.0, WASH_MIDDLE];
/// How far each try moves, and how many there are at most (enough to reach
/// black, white or grey from anywhere).
const STEP: f32 = 0.02;
const TRIES: usize = 60;

/// One mode's grounds, as the theme draws them, that the derived colours
/// are drawn with.
#[derive(Debug, Clone)]
pub struct Surfaces {
    pub dark: bool,
    /// The page, under the wash.
    pub page: Color,
    pub text: Color,
    /// Secondary text: the text colour, faded.
    pub secondary: Color,
    /// The sidebar, the side panel and the player bar: the tint goes into them.
    pub shell: Vec<Color>,
    /// The other grounds accent text sits on: a card, a tonal button resting
    /// and hovered, a hovered row, the selected row.
    pub grounds: Vec<Color>,
    /// The quiet buttons' veils over the wash, resting and hovered.
    pub veils: [Color; 2],
}

/// A cover's colours for one mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Derived {
    /// The page's wash, drawn at `WASH_TOP` and fading out.
    pub wash: Color,
    /// What goes into the shell's grounds, at `TINT_DARK` or `TINT_LIGHT`.
    pub tint: Color,
    pub accent: Color,
    /// A filled button's label on the accent: black or white.
    pub label: Color,
}

impl Derived {
    /// The colours `main` (a cover's main colour, with a hue) gives on
    /// `surfaces`.
    pub fn new(main: Hsv, surfaces: &Surfaces) -> Self {
        let wash = wash(main, surfaces);
        let tint = tint(main, surfaces);
        let mut derived = Self {
            wash,
            tint,
            accent: Color::BLACK,
            label: Color::WHITE,
        };
        let grounds = derived.accent_grounds(surfaces);
        derived.accent = accent(main, surfaces.dark, &grounds);
        derived.label = label(derived.accent);
        derived
    }

    /// The wash at `strength`, drawn over the page.
    pub fn wash_over(&self, page: Color, strength: f32) -> Color {
        over(self.wash.scale_alpha(strength), page)
    }

    /// A shell ground with the tint in it.
    pub fn tinted(&self, ground: Color, dark: bool) -> Color {
        let amount = if dark { TINT_DARK } else { TINT_LIGHT };
        over(self.tint.scale_alpha(amount), ground)
    }

    /// Every ground the accent is drawn on, as the screen shows it: the
    /// page, the cards and buttons, the tinted shell, the wash, and a veil
    /// on the wash.
    pub fn accent_grounds(&self, surfaces: &Surfaces) -> Vec<Color> {
        let mut grounds = vec![surfaces.page];
        grounds.extend(&surfaces.grounds);
        grounds.extend(
            surfaces
                .shell
                .iter()
                .map(|g| self.tinted(*g, surfaces.dark)),
        );
        for strength in WASH_STEPS {
            grounds.push(self.wash_over(surfaces.page, strength));
        }
        let top = self.wash_over(surfaces.page, WASH_TOP);
        grounds.extend(surfaces.veils.iter().map(|veil| over(*veil, top)));
        grounds
    }
}

/// Whether text and secondary text read on `ground`.
fn words_read(surfaces: &Surfaces, ground: Color) -> bool {
    contrast(surfaces.text, ground) >= TEXT && contrast(surfaces.secondary, ground) >= TEXT
}

/// The wash: the cover's hue, lifted in saturation as the iPhone app does
/// (spread over a header, a colour at its own saturation reads washed out),
/// then darkened (dark mode) or paled (light) until the page's text reads
/// over it at its strongest.
fn wash(main: Hsv, surfaces: &Surfaces) -> Color {
    let reads = |colour: Color| {
        WASH_STEPS
            .iter()
            .all(|&s| words_read(surfaces, over(colour.scale_alpha(s), surfaces.page)))
    };
    if surfaces.dark {
        // From the brightest the iPhone app allows, down: on a big screen
        // the wash may be as bold as the words on it allow.
        let s = (main.s * 1.35).clamp(0.42, 0.78);
        search(|i| main.with(s, 0.55 - STEP * i).colour(), reads)
    } else {
        let s = (main.s * 1.2).clamp(0.30, 0.65);
        search(|i| main.with(s - STEP * i, 0.95).colour(), reads)
    }
}

/// The tint: the cover's hue, saturated, that the shell's text still reads
/// on once mixed in.
fn tint(main: Hsv, surfaces: &Surfaces) -> Color {
    let s = main.s.clamp(0.45, 0.75);
    let reads = |colour: Color| {
        let derived = Derived {
            wash: Color::TRANSPARENT,
            tint: colour,
            accent: Color::BLACK,
            label: Color::WHITE,
        };
        surfaces
            .shell
            .iter()
            .all(|g| words_read(surfaces, derived.tinted(*g, surfaces.dark)))
    };
    if surfaces.dark {
        search(|i| main.with(s, 0.75 - STEP * i).colour(), reads)
    } else {
        search(|i| main.with(s - STEP * i, 0.9).colour(), reads)
    }
}

/// The accent: the cover's hue, saturated, made lighter (dark mode, by
/// giving up saturation at full value) or darker (light) until it reads on
/// every ground and carries a label.
fn accent(main: Hsv, dark: bool, grounds: &[Color]) -> Color {
    let reads = |colour: Color| {
        let label = label(colour);
        grounds.iter().all(|g| contrast(colour, *g) >= TEXT)
            && contrast(label, colour) >= TEXT
            && contrast(label, deviate(colour, 0.1)) >= TEXT
    };
    if dark {
        let s = main.s.clamp(0.40, 0.75);
        search(|i| main.with(s - STEP * i, 1.0).colour(), reads)
    } else {
        let s = main.s.clamp(0.55, 0.90);
        search(|i| main.with(s, 0.8 - STEP * i).colour(), reads)
    }
}

/// White or black, whichever reads better on the accent and on the shade
/// iced draws a hovered filled button in.
pub fn label(accent: Color) -> Color {
    let hovered = deviate(accent, 0.1);
    let worst = |label: Color| contrast(label, accent).min(contrast(label, hovered));
    if worst(Color::WHITE) >= worst(Color::BLACK) {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

/// The first of `TRIES` candidates (`candidate(0)`, `candidate(1)`…) that
/// `reads`, else the last, which every caller makes black, white or grey
/// enough to read anyway.
fn search(candidate: impl Fn(f32) -> Color, reads: impl Fn(Color) -> bool) -> Color {
    let mut last = candidate(0.0);
    for i in 0..TRIES {
        // A small count, exactly a float.
        #[allow(clippy::cast_precision_loss)]
        let colour = candidate(i as f32);
        if reads(colour) {
            return colour;
        }
        last = colour;
    }
    last
}
