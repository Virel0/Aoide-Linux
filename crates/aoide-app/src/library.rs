//! Library views' shared pieces: tiles.

use aoide_melete::Item;

use crate::artwork::ArtKey;

/// An album (or artist) as a tile in a grid or a shelf.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub art: Option<ArtKey>,
    /// Drawn round, as Apple Music shows artists.
    pub round: bool,
}

impl Tile {
    pub fn album(item: &Item) -> Self {
        let subtitle = item
            .album_artist
            .clone()
            .or_else(|| item.artists.first().cloned())
            .unwrap_or_default();
        Self {
            id: item.id.clone(),
            title: item.name.clone(),
            subtitle,
            art: item.artwork_source().map(|(id, tag)| ArtKey::new(id, tag)),
            round: false,
        }
    }

    /// An artist. Melete has no pictures of artists, so a round placeholder
    /// stands in.
    pub fn artist(item: &Item) -> Self {
        let subtitle = match item.child_count {
            Some(1) => "1 album".to_owned(),
            Some(n) if n > 1 => format!("{n} albums"),
            _ => String::new(),
        };
        Self {
            id: item.id.clone(),
            title: item.name.clone(),
            subtitle,
            art: None,
            round: true,
        }
    }

    /// The album a track belongs to, as a tile: what Home's most played shelf
    /// shows, since plays are counted on tracks.
    pub fn album_of_track(item: &Item) -> Option<Self> {
        let album_id = item.album_id.clone()?;
        Some(Self {
            id: album_id.clone(),
            title: item.album.clone().unwrap_or_else(|| item.name.clone()),
            subtitle: item
                .album_artist
                .clone()
                .or_else(|| item.artists.first().cloned())
                .unwrap_or_default(),
            art: item
                .has_cover
                .then(|| ArtKey::new(album_id, aoide_melete::COVER_TAG)),
            round: false,
        })
    }
}

/// The albums of the tracks, each once, in the order first met.
pub fn albums_of_tracks(tracks: &[Item], limit: usize) -> Vec<Tile> {
    let mut seen = std::collections::HashSet::new();
    tracks
        .iter()
        .filter_map(Tile::album_of_track)
        .filter(|tile| seen.insert(tile.id.clone()))
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{COVER_TAG, ItemKind};

    fn track(id: &str, album: Option<(&str, &str)>) -> Item {
        Item {
            id: id.into(),
            name: id.into(),
            album_id: album.map(|(id, _)| id.into()),
            album: album.map(|(_, name)| name.into()),
            has_cover: true,
            ..Item::default()
        }
    }

    #[test]
    fn played_tracks_become_their_albums_once_each() {
        let tracks = [
            track("t1", Some(("A", "Alpha"))),
            track("t2", Some(("B", "Beta"))),
            track("t3", Some(("A", "Alpha"))),
            track("t4", None),
        ];
        let tiles = albums_of_tracks(&tracks, 10);
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert_eq!(tiles[0].art, Some(ArtKey::new("A", COVER_TAG)));
        assert!(!tiles[0].round);
    }

    #[test]
    fn an_artist_tile_is_round_with_a_placeholder() {
        let artist = Tile::artist(&Item {
            id: "1".into(),
            name: "Mara Vey".into(),
            kind: ItemKind::Artist,
            child_count: Some(2),
            ..Item::default()
        });
        assert!(artist.round);
        assert_eq!(artist.art, None);
        assert_eq!(artist.subtitle, "2 albums");
        let album = Tile::album(&Item {
            id: "2".into(),
            name: "Low Tide Radio".into(),
            kind: ItemKind::Album,
            album_id: Some("2".into()),
            has_cover: true,
            ..Item::default()
        });
        assert!(!album.round);
        assert_eq!(album.art, Some(ArtKey::new("2", COVER_TAG)));
    }
}
