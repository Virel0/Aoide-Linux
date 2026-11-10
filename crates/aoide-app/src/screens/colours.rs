//! Which pages take their colours from a cover, and from where.

use super::Page;
use crate::cover_colour::Origin;

impl Page {
    /// Where the page's colours come from, in order (`cover_colour`): an
    /// album's, an Aoide playlist's, an artist's and a web collection's
    /// page, once loaded. Empty for every other page, which keeps the
    /// usual look.
    pub fn colour_origins(&self) -> Vec<Origin> {
        match self {
            Page::Album(state) => state.colour_origins(),
            Page::Artist(state) => state.colour_origins(),
            Page::Playlist(state) => state.colour_origins(),
            Page::WebCollection(state) => state.colour_origins(),
            _ => Vec::new(),
        }
    }

    /// Albums' colours from the server that came with the page, so they
    /// aren't asked for again.
    pub fn known_palettes(&self) -> Vec<(&str, &[[u8; 3]])> {
        match self {
            Page::Album(state) => state.known_palette().into_iter().collect(),
            Page::Artist(state) => state.known_palettes().collect(),
            _ => Vec::new(),
        }
    }
}
