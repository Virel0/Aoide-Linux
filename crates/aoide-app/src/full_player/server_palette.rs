//! The cover's main colours as Melete works them out (an album's
//! `colours.palette`), so the moving ground is drawn in the colours the
//! iPhone app uses. Asked for once per album while the full player is open;
//! an album with none, or a server that doesn't answer, keeps the colours
//! found in the blurred cover here.

use iced::Task;

use super::FullPlayer;
use super::backdrop::to_linear;
use crate::app::{App, Message};
use crate::listening::Msg;

/// How many colours the grounds draw with.
const COLOURS: usize = 4;

impl App {
    /// Asks the server for the playing song's album's colours, once per
    /// album, while the full player is open.
    pub(crate) fn reconcile_palette(&mut self) -> Task<Message> {
        let album = self
            .playback
            .current()
            .and_then(|(_, q)| q.meta.album_id.clone());
        let (Some(album), Some(full), Some(signed)) =
            (album, self.listening.full.as_mut(), self.session.as_ref())
        else {
            return Task::none();
        };
        if full.palettes.contains_key(&album) {
            return Task::none();
        }
        full.palettes.insert(album.clone(), Vec::new());
        let client = signed.connection.client.clone();
        Task::perform(
            async move {
                match client.album(&album).await {
                    Ok((item, _)) => (album, item.palette),
                    Err(_) => (album, Vec::new()),
                }
            },
            |(album, palette)| Message::Listening(Msg::AlbumPalette(album, palette)),
        )
    }

    pub(crate) fn palette_arrived(
        &mut self,
        album: String,
        palette: Vec<[u8; 3]>,
    ) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut() {
            full.palettes.insert(album, palette);
        }
        Task::none()
    }
}

impl FullPlayer {
    /// The colours the ground draws with: the server's for the album whose
    /// cover it is, else those found in the blurred cover (`found`).
    pub(super) fn ground_palette(&self, found: [[f32; 3]; COLOURS]) -> [[f32; 3]; COLOURS] {
        self.backdrop_for
            .as_ref()
            .and_then(|key| self.palettes.get(&key.item))
            .and_then(|palette| linear(palette))
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
        assert_eq!(full.ground_palette(FOUND), FOUND, "no cover yet");
        full.backdrop_for = Some(ArtKey::sized("7", "cover", crate::artwork::ROW));
        assert_eq!(full.ground_palette(FOUND), FOUND, "not asked yet");
        full.palettes.insert("7".into(), Vec::new());
        assert_eq!(full.ground_palette(FOUND), FOUND, "asked, or none to give");
        full.palettes.insert("7".into(), vec![[0, 0, 0]]);
        assert_eq!(full.ground_palette(FOUND), [[0.0; 3]; COLOURS]);
    }
}
