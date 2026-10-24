//! Library views' shared pieces: tiles.

use aoide_jellyfin::Item;

use crate::artwork::ArtKey;

/// An album (or artist) as a tile in a grid or a shelf.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub art: Option<ArtKey>,
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
        }
    }

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
            art: item.artwork_source().map(|(id, tag)| ArtKey::new(id, tag)),
        }
    }

    /// The album a track belongs to, as a tile: what Home's played shelves show,
    /// since Jellyfin keeps play statistics on tracks only.
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
                .album_primary_image_tag
                .clone()
                .map(|tag| ArtKey::new(album_id, tag)),
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

    fn item(json: serde_json::Value) -> Item {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn played_tracks_become_their_albums_once_each() {
        let tracks = [
            item(
                serde_json::json!({"Id": "t1", "Name": "a", "AlbumId": "A", "Album": "Alpha", "AlbumPrimaryImageTag": "x"}),
            ),
            item(serde_json::json!({"Id": "t2", "Name": "b", "AlbumId": "B", "Album": "Beta"})),
            item(serde_json::json!({"Id": "t3", "Name": "c", "AlbumId": "A", "Album": "Alpha"})),
            item(serde_json::json!({"Id": "t4", "Name": "loose"})),
        ];
        let tiles = albums_of_tracks(&tracks, 10);
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert_eq!(tiles[0].art, Some(ArtKey::new("A", "x")));
        assert_eq!(tiles[1].art, None);
    }
}
