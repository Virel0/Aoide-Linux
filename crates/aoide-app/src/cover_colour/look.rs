//! A cover's look: its main colour, picked from its palette, made into the
//! derived colours for each mode.

use iced::Theme;

use super::derive::Derived;
use super::maths::Hsv;

/// Below this saturation a colour is a grey: white, black or in between.
const GREY: f32 = 0.16;
/// Below this value a colour is too near black to name.
const BLACK: f32 = 0.12;
/// How much less each later palette colour counts (they cover less of the
/// cover), against how saturated it is.
const LATER: f32 = 0.12;

/// The colours a cover gives the app, in light mode and in dark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub light: Derived,
    pub dark: Derived,
}

impl Look {
    /// The look of a cover whose main colours are `palette` (sRGB, the most
    /// of the cover first); `None` when none of them has a hue to speak of,
    /// so a grey, black or white cover leaves the app as it is.
    pub fn of(palette: &[[u8; 3]]) -> Option<Self> {
        let main = main_colour(palette)?;
        Some(Self {
            light: Derived::new(main, &crate::theme::artwork::surfaces(false)),
            dark: Derived::new(main, &crate::theme::artwork::surfaces(true)),
        })
    }

    /// The colours for `theme`'s mode.
    pub fn for_theme(&self, theme: &Theme) -> Derived {
        self.in_mode(theme.extended_palette().is_dark)
    }

    pub fn in_mode(&self, dark: bool) -> Derived {
        if dark { self.dark } else { self.light }
    }
}

/// The colour someone would name the cover by: of the palette's colours
/// that aren't grey or near black, the most saturated, a colour covering
/// more counting for more (as the iPhone app ranks a small vivid area over a
/// large muted one); `None` when every one is grey.
pub fn main_colour(palette: &[[u8; 3]]) -> Option<Hsv> {
    palette
        .iter()
        .map(|&bytes| Hsv::of(bytes))
        .enumerate()
        .filter(|(_, c)| c.s >= GREY && c.v >= BLACK)
        .map(|(rank, c)| {
            // At most five colours: exactly a float.
            #[allow(clippy::cast_precision_loss)]
            let weight = c.s * c.s * (1.0 - LATER * rank as f32).max(0.1);
            (weight, c)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greys_whites_and_blacks_have_no_main_colour() {
        assert_eq!(main_colour(&[]), None);
        assert_eq!(main_colour(&[[128, 128, 128], [250, 250, 250]]), None);
        assert_eq!(main_colour(&[[8, 6, 10], [20, 20, 22]]), None);
        // A warm off-white is still white.
        assert_eq!(main_colour(&[[245, 240, 232]]), None);
        assert!(Look::of(&[[30, 30, 30], [200, 200, 200]]).is_none());
    }

    #[test]
    fn a_vivid_colour_beats_a_larger_muted_one() {
        // Mostly a dusty grey-blue, with a strong red.
        let main = main_colour(&[[110, 120, 135], [210, 30, 40]]).expect("a colour");
        assert!(main.h < 0.02 || main.h > 0.98, "{main:?}");
        // Mostly black, with a gold: the gold.
        let gold = main_colour(&[[10, 10, 10], [200, 160, 40]]).expect("a colour");
        assert!((0.08..0.16).contains(&gold.h), "{gold:?}");
    }

    #[test]
    fn of_two_alike_the_one_covering_more_wins() {
        let main = main_colour(&[[40, 80, 200], [200, 40, 80]]).expect("a colour");
        assert!((0.55..0.7).contains(&main.h), "{main:?}");
    }
}
