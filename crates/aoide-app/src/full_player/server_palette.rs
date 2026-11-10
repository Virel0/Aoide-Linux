//! The cover's main colours as Melete works them out (an album's
//! `colours.palette`), so the moving ground is drawn in the colours the
//! iPhone app uses. Asked for once per album (`cover_colour`); an album with
//! none, or a server that doesn't answer, keeps the colours found in the
//! blurred cover here.

use super::FullPlayer;
use super::backdrop::to_linear;
use crate::cover_colour::CoverColours;

/// How many colours the grounds draw with.
const COLOURS: usize = 4;

impl FullPlayer {
    /// The colours the ground draws with: the server's for the album whose
    /// cover it is, else those found in the blurred cover (`found`).
    pub(super) fn ground_palette(
        &self,
        found: [[f32; 3]; COLOURS],
        colours: &CoverColours,
    ) -> [[f32; 3]; COLOURS] {
        self.backdrop_for
            .as_ref()
            .and_then(|key| colours.album_palette(&key.item))
            .and_then(linear)
            .unwrap_or(found)
    }
}

/// `palette` (sRGB, the most of the cover first) as four colours in linear
/// light, repeating from the first when there are fewer; `None` when empty.
fn linear(palette: &[[u8; 3]]) -> Option<[[f32; 3]; COLOURS]> {
    if palette.is_empty() {
        return None;
    }
    Some(std::array::from_fn(|i| {
        palette[i % palette.len()].map(to_linear)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork::ArtKey;
    use crate::cover_colour::Origin;
    use std::time::Instant;

    const FOUND: [[f32; 3]; COLOURS] = [[0.5; 3]; COLOURS];

    #[test]
    fn the_servers_colours_come_in_linear_light_and_fill_four() {
        let four = linear(&[[255, 0, 0], [0, 0, 255]]).unwrap();
        assert_eq!(four[0], [1.0, 0.0, 0.0]);
        assert_eq!(four[1], [0.0, 0.0, 1.0]);
        assert_eq!(four[2], four[0]);
        assert_eq!(four[3], four[1]);
        // sRGB 128 is about 0.22 in linear light, not 0.5.
        let grey = linear(&[[128, 128, 128]]).unwrap()[0][0];
        assert!((grey - 0.216).abs() < 0.001, "{grey}");
        assert_eq!(linear(&[]), None);
    }

    #[test]
    fn the_ground_uses_the_servers_colours_else_the_ones_found_here() {
        let mut full = FullPlayer::new(Instant::now());
        let mut colours = CoverColours::default();
        assert_eq!(full.ground_palette(FOUND, &colours), FOUND, "no cover yet");
        full.backdrop_for = Some(ArtKey::sized("7", "cover", crate::artwork::ROW));
        assert_eq!(full.ground_palette(FOUND, &colours), FOUND, "not asked yet");
        colours.asking(Origin::Album("7".into()));
        assert_eq!(full.ground_palette(FOUND, &colours), FOUND, "asked");
        colours.arrived(Origin::Album("7".into()), Vec::new());
        assert_eq!(full.ground_palette(FOUND, &colours), FOUND, "none to give");
        colours.arrived(Origin::Album("7".into()), vec![[0, 0, 0]]);
        assert_eq!(full.ground_palette(FOUND, &colours), [[0.0; 3]; COLOURS]);
    }
}
