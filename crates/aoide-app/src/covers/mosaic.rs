//! A smart playlist's cover: the covers of the first four different albums
//! its rules match, in a 2×2 mosaic. Worked out on this computer whenever
//! the playlist's cover is looked for (its page opens, the playlists are
//! read again after a sync or a change), so it follows the songs; drawn from
//! the albums' covers in the artwork cache. Nothing is uploaded or synced.

use std::collections::HashMap;

use aoide_store::{Calendar, Playlist, PlaylistCover, Store};

use crate::artwork::ArtKey;

/// How many albums make a mosaic.
const ALBUMS: usize = 4;

/// What a playlist's cover is drawn as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cover {
    /// One picture: a chosen one, an album's, or a stand-in.
    One(ArtKey),
    /// Four albums' covers, left to right, then top to bottom. Boxed, so a
    /// cover is no larger than one key wherever it is kept.
    Mosaic(Box<[ArtKey; ALBUMS]>),
}

impl Cover {
    /// The covers to have in the artwork cache.
    pub fn keys(&self) -> &[ArtKey] {
        match self {
            Self::One(key) => std::slice::from_ref(key),
            Self::Mosaic(keys) => keys.as_slice(),
        }
    }

    /// A smart playlist's cover from its albums: four make a mosaic; one to
    /// three, the first one's cover (a mosaic with gaps or repeats looks
    /// broken); none, nothing.
    pub fn from_albums(albums: &[String]) -> Option<Self> {
        let keys: Vec<ArtKey> = albums
            .iter()
            .take(ALBUMS)
            .map(|album| ArtKey::new(album, aoide_melete::COVER_TAG))
            .collect();
        match <[ArtKey; ALBUMS]>::try_from(keys) {
            Ok(four) => Some(Self::Mosaic(Box::new(four))),
            Err(fewer) => fewer.into_iter().next().map(Self::One),
        }
    }
}

/// The albums of each smart playlist nobody chose a cover for, as of `now`.
/// A chosen picture or album still wins; a playlist whose rules won't read
/// is left to its stand-in.
pub fn smart_albums(
    store: &Store,
    lists: &[Playlist],
    covers: &HashMap<String, PlaylistCover>,
    now: aoide_store::Millis,
) -> HashMap<String, Vec<String>> {
    lists
        .iter()
        .filter(|list| list.is_smart && !chosen(covers.get(&list.id)))
        .filter_map(|list| {
            let albums = store
                .smart_cover_albums(list, now, Calendar::Local, ALBUMS)
                .ok()?;
            Some((list.id.clone(), albums))
        })
        .collect()
}

/// A cover somebody chose, rather than a stand-in.
fn chosen(cover: Option<&PlaylistCover>) -> bool {
    matches!(
        cover,
        Some(PlaylistCover::Image { .. } | PlaylistCover::Album(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn albums(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn key(album: &str) -> ArtKey {
        ArtKey::new(album, aoide_melete::COVER_TAG)
    }

    #[test]
    fn four_albums_make_a_mosaic_and_fewer_the_first_ones_cover() {
        assert_eq!(
            Cover::from_albums(&albums(&["a", "b", "c", "d", "e"])),
            Some(Cover::Mosaic(Box::new([
                key("a"),
                key("b"),
                key("c"),
                key("d")
            ])))
        );
        assert_eq!(
            Cover::from_albums(&albums(&["a", "b", "c"])),
            Some(Cover::One(key("a")))
        );
        assert_eq!(
            Cover::from_albums(&albums(&["a"])),
            Some(Cover::One(key("a")))
        );
        assert_eq!(Cover::from_albums(&[]), None);
    }

    #[test]
    fn a_chosen_cover_beats_the_mosaic() {
        let store = Store::in_memory("desk").unwrap();
        let now = aoide_store::now_millis();
        let mut row = aoide_store::TrackRef::new("t1", "k1", "Lantern", "Mara Vey", "Saltwater", 1);
        row.album_id = Some("al1".into());
        store.cache_tracks(&[row]).unwrap();
        let every =
            aoide_store::SmartRules::new(aoide_store::Match::All, vec![], None, None).unwrap();
        let smart = |name: &str| {
            let list = store.create_playlist(name, None, None, now).unwrap();
            store
                .record(list.setting_smart_rules(&every), false, now)
                .unwrap()
        };
        let (open, picked) = (smart("Open"), smart("Picked"));
        let plain = store.create_playlist("Plain", None, None, now).unwrap();
        let covers = HashMap::from([(picked.id.clone(), PlaylistCover::Album("al9".into()))]);
        let found = smart_albums(&store, &[open.clone(), picked, plain], &covers, now);
        assert_eq!(found, HashMap::from([(open.id, albums(&["al1"]))]));
    }
}
