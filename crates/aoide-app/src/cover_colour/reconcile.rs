//! Asking for the colours the window needs: the playing song's (for the
//! window's accent and tint, and the full player's ground) and the open
//! page's, each once.

use iced::Task;
use iced::widget::image::Handle;

use super::{CoverColours, Look, Msg, Origin, origins_of};
use crate::app::{App, Message};
use crate::appearance::Accent;
use crate::artwork::ArtKey;

impl App {
    /// Whether the window takes its colours from the artwork.
    pub(crate) fn colours_from_artwork(&self) -> bool {
        self.config.interface.accent == Accent::Artwork
    }

    /// Where the playing song's colours come from: its album's, then its
    /// cover's as the player bar draws it.
    fn playing_origins(&self) -> Vec<Origin> {
        self.playback
            .current()
            .and_then(|(_, q)| q.meta.artwork.as_ref())
            .map(|(item, tag)| origins_of(&ArtKey::sized(item, tag, crate::artwork::ROW)))
            .unwrap_or_default()
    }

    /// The playing song's look, when the window takes its colours from the
    /// artwork and they are here.
    pub(crate) fn playing_look(&self) -> Option<Look> {
        if !self.colours_from_artwork() {
            return None;
        }
        self.colours.look(&self.playing_origins())
    }

    /// The open page's look, likewise.
    pub(crate) fn page_look(&self, page: &crate::screens::Page) -> Option<Look> {
        if !self.colours_from_artwork() {
            return None;
        }
        self.colours.look(&page.colour_origins())
    }

    /// Keeps what the open page brought, then asks for what the playing
    /// song and the page need and nobody has asked for. The full player's
    /// ground needs the playing album's colours whatever the setting.
    pub(crate) fn reconcile_colours(&mut self) -> Task<Message> {
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let page = signed.nav.as_ref().map(|nav| &nav.current().page);
        if let Some(page) = page {
            for (album, palette) in page.known_palettes() {
                self.colours.knows(album, palette);
            }
        }
        let mut wanted = Vec::new();
        if self.colours_from_artwork() {
            wanted.push(self.playing_origins());
            wanted.extend(page.map(crate::screens::Page::colour_origins));
        } else if self.listening.full.is_some() {
            wanted.push(self.playing_origins());
            wanted[0].retain(|origin| matches!(origin, Origin::Album(_)));
        }
        let client = signed.connection.client.clone();
        let mut tasks = Vec::new();
        for origins in &wanted {
            let Some(origin) = self.colours.next_to_ask(origins).cloned() else {
                continue;
            };
            if let Some(task) = ask(&mut self.colours, &self.artwork, &client, origin) {
                tasks.push(task);
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn cover_colours(&mut self, message: Msg) -> Task<Message> {
        match message {
            Msg::Arrived(origin, palette) => self.colours.arrived(origin, palette),
        }
        Task::none()
    }
}

/// Asks for `origin`'s colours: an album's of the server, a picture's found
/// in it off the interface's thread once the picture is here (`None`
/// until then).
fn ask(
    colours: &mut CoverColours,
    artwork: &crate::artwork::Artwork,
    client: &aoide_melete::MeleteClient,
    origin: Origin,
) -> Option<Task<Message>> {
    let arrived = |origin, palette| Message::CoverColours(Msg::Arrived(origin, palette));
    match &origin {
        Origin::Album(id) => {
            let (client, id) = (client.clone(), id.clone());
            colours.asking(origin.clone());
            Some(Task::perform(
                async move {
                    // No colours is an answer too: the picture is tried next.
                    client
                        .album(&id)
                        .await
                        .map(|(item, _)| item.palette)
                        .unwrap_or_default()
                },
                move |palette| arrived(origin.clone(), palette),
            ))
        }
        Origin::Picture(key) => {
            let Some(Handle::Bytes(_, bytes)) = artwork.get(key) else {
                return None;
            };
            let bytes = bytes.clone();
            colours.asking(origin.clone());
            Some(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || crate::full_player::picture_palette(&bytes))
                        .await
                        .unwrap_or_default()
                },
                move |palette| arrived(origin.clone(), palette),
            ))
        }
    }
}
