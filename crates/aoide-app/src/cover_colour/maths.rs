//! The arithmetic of colour the derivation needs: WCAG 2's contrast, one
//! colour drawn over another, and hue, saturation and value.

use iced::Color;

/// WCAG 2's relative luminance of an opaque colour.
pub fn luminance(colour: Color) -> f32 {
    let channel = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(colour.r) + 0.7152 * channel(colour.g) + 0.0722 * channel(colour.b)
}

/// `fg` drawn over the opaque `bg`, as the screen shows it.
pub fn over(fg: Color, bg: Color) -> Color {
    let mix = |f: f32, b: f32| f * fg.a + b * (1.0 - fg.a);
    Color::from_rgb(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
}

/// WCAG 2's contrast ratio of `fg` (which may be translucent) drawn over the
/// opaque `bg`.
pub fn contrast(fg: Color, bg: Color) -> f32 {
    let (a, b) = (luminance(over(fg, bg)), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// A colour as hue (0 to 1, round the wheel), saturation and value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    /// An sRGB colour, as bytes, in hue, saturation and value.
    pub fn of([r, g, b]: [u8; 3]) -> Self {
        let [r, g, b] = [r, g, b].map(|c| f32::from(c) / 255.0);
        let max = r.max(g).max(b);
        let delta = max - r.min(g).min(b);
        let h = if delta <= 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        } / 6.0;
        let s = if max <= 0.0 { 0.0 } else { delta / max };
        Self { h, s, v: max }
    }

    /// The same hue at another saturation and value, each held to 0..=1.
    pub fn with(self, s: f32, v: f32) -> Self {
        Self {
            h: self.h,
            s: s.clamp(0.0, 1.0),
            v: v.clamp(0.0, 1.0),
        }
    }

    pub fn colour(self) -> Color {
        let Self { h, s, v } = self;
        let sector = (h.rem_euclid(1.0) * 6.0).min(5.999_999);
        let f = sector.fract();
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        // The sector is 0..6, so the cast picks one of six.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (r, g, b) = match sector as u8 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color::from_rgb(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        [(a.r, b.r), (a.g, b.g), (a.b, b.b)]
            .iter()
            .all(|(x, y)| (x - y).abs() < 0.01)
    }

    #[test]
    fn black_on_white_is_twenty_one_to_one() {
        let ratio = contrast(Color::BLACK, Color::WHITE);
        assert!((ratio - 21.0).abs() < 0.01, "{ratio}");
        assert!((contrast(Color::WHITE, Color::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn a_translucent_colour_is_drawn_over_its_ground() {
        let half = over(Color::WHITE.scale_alpha(0.5), Color::BLACK);
        assert!(close(half, Color::from_rgb(0.5, 0.5, 0.5)));
    }

    #[test]
    fn hue_saturation_and_value_go_there_and_back() {
        for bytes in [[200, 30, 40], [20, 160, 60], [40, 60, 220], [250, 220, 10]] {
            let hsv = Hsv::of(bytes);
            let back = hsv.colour();
            let want = Color::from_rgb8(bytes[0], bytes[1], bytes[2]);
            assert!(close(back, want), "{bytes:?}: {back:?}");
        }
        let red = Hsv::of([255, 0, 0]);
        assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
        assert_eq!(Hsv::of([128, 128, 128]).s, 0.0);
    }
}
