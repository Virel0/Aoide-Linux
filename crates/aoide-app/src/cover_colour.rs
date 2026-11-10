//! Colour from the artwork: each cover's main colours, from Melete (an
//! album's `colours.palette`) or, where the server has none, found in the
//! picture here; and what they become (`look`, `derive`): a page's wash and
//! accent, and the window's tint and accent from what is playing. Asked for
//! once per album or picture and kept for the run.

mod derive;
mod look;
mod maths;
mod reconcile;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use maths::{contrast, over};
#[cfg(test)]
pub(crate) use tests::COLOURFUL;

use std::collections::HashMap;

use crate::artwork::ArtKey;

pub use derive::{Derived, Surfaces, WASH_MIDDLE, WASH_TOP};
pub use look::Look;

/// Where colours can come from, tried in order: an album's, from the server,
/// or a picture's, found in it here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Album(String),
    Picture(ArtKey),
}

/// What came back for an origin.
#[derive(Debug, Clone, PartialEq)]
struct Known {
    /// The main colours, the most of the cover first; empty when it has none.
    palette: Vec<[u8; 3]>,
    /// `None` when they are all grey.
    look: Option<Look>,
}

/// Albums' and pictures' colours: `None` while being asked for.
#[derive(Debug, Default)]
pub struct CoverColours {
    albums: HashMap<String, Option<Known>>,
    pictures: HashMap<ArtKey, Option<Known>>,
}

/// Where an origin stands.
enum Standing<'a> {
    Unasked,
    Asking,
    Known(&'a Known),
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// An origin's colours (empty when it has none, or couldn't be read).
    Arrived(Origin, Vec<[u8; 3]>),
}

impl CoverColours {
    fn standing(&self, origin: &Origin) -> Standing<'_> {
        let entry = match origin {
            Origin::Album(id) => self.albums.get(id),
            Origin::Picture(key) => self.pictures.get(key),
        };
        match entry {
            None => Standing::Unasked,
            Some(None) => Standing::Asking,
            Some(Some(known)) => Standing::Known(known),
        }
    }

    /// The look of the first of `origins` with colours. An origin with none
    /// passes to the next; while one is still being asked for there is no
    /// look yet, so a later origin's colours don't show and then change.
    pub fn look(&self, origins: &[Origin]) -> Option<Look> {
        for origin in origins {
            match self.standing(origin) {
                Standing::Known(known) if known.palette.is_empty() => {}
                Standing::Known(known) => return known.look,
                Standing::Unasked | Standing::Asking => return None,
            }
        }
        None
    }

    /// The origin to ask for next to find `origins`' look, if any.
    pub fn next_to_ask<'a>(&self, origins: &'a [Origin]) -> Option<&'a Origin> {
        for origin in origins {
            match self.standing(origin) {
                Standing::Known(known) if known.palette.is_empty() => {}
                Standing::Unasked => return Some(origin),
                Standing::Known(_) | Standing::Asking => return None,
            }
        }
        None
    }

    /// An album's colours from the server, when it gave some.
    pub fn album_palette(&self, album: &str) -> Option<&[[u8; 3]]> {
        match self.albums.get(album) {
            Some(Some(known)) if !known.palette.is_empty() => Some(&known.palette),
            _ => None,
        }
    }

    /// `origin` is being asked for.
    pub fn asking(&mut self, origin: Origin) {
        match origin {
            Origin::Album(id) => self.albums.insert(id, None),
            Origin::Picture(key) => self.pictures.insert(key, None),
        };
    }

    /// `origin`'s colours arrived.
    pub fn arrived(&mut self, origin: Origin, palette: Vec<[u8; 3]>) {
        let known = Some(Known {
            look: Look::of(&palette),
            palette,
        });
        match origin {
            Origin::Album(id) => self.albums.insert(id, known),
            Origin::Picture(key) => self.pictures.insert(key, known),
        };
    }

    /// An album's colours that came with something else (its page, an
    /// artist's albums): kept unless already known.
    pub fn knows(&mut self, album: &str, palette: &[[u8; 3]]) {
        if !matches!(self.albums.get(album), Some(Some(_))) {
            self.arrived(Origin::Album(album.to_owned()), palette.to_vec());
        }
    }
}

/// The origins of a cover drawn under `key`: the album's colours from the
/// server first when it is an album's cover, then the picture's own.
pub fn origins_of(key: &ArtKey) -> Vec<Origin> {
    let mut origins = Vec::with_capacity(2);
    if key.tag == aoide_melete::COVER_TAG {
        origins.push(Origin::Album(key.item.clone()));
    }
    origins.push(Origin::Picture(key.clone()));
    origins
}
