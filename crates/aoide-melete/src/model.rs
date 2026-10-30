use std::time::Duration;

use serde::{Deserialize, Deserializer};

/// The kinds of item a music client meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ItemKind {
    /// A track.
    #[default]
    Track,
    /// An album.
    Album,
    /// An artist, credited on a track or an album.
    Artist,
    /// A genre. Melete names genres rather than giving them ids, so its id is its
    /// name.
    Genre,
}

/// A track, album, artist or genre, in the one shape the app's lists draw.
/// Built from Melete's own types (`MusicTrack`, `MusicAlbum`, `MusicArtist`,
/// `MusicGenre`), which each fill in what they have.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    /// The server's id for the item, as text (Melete's ids are numbers). What
    /// every per-item call takes.
    pub id: String,
    /// The title of a track or album, the name of an artist or genre.
    pub name: String,
    /// What kind of item this is.
    pub kind: ItemKind,
    /// A track's album title.
    pub album: Option<String>,
    /// A track's album's [`id`](Self::id); an album's own.
    pub album_id: Option<String>,
    /// The album artist as one name: a track's album's, an album's own.
    pub album_artist: Option<String>,
    /// The album artist with its id, when there's one artist to name (not a
    /// compilation).
    pub album_artists: Vec<NameId>,
    /// A track's artists, in credit order (a featured artist after the main one).
    pub artists: Vec<String>,
    /// The same artists with their ids.
    pub artist_items: Vec<NameId>,
    /// Genres in the file's tag order.
    pub genres: Vec<String>,
    /// A track's or album's length, when known.
    pub duration: Option<Duration>,
    /// Track number within its disc.
    pub track_number: Option<i32>,
    /// Disc number within its album.
    pub disc_number: Option<i32>,
    /// The release year.
    pub year: Option<i32>,
    /// An album's tracks, an artist's albums, a genre's tracks.
    pub child_count: Option<i32>,
    /// Whether the album has a cover: always for a track, whose album may or may
    /// not (Melete doesn't say on the track).
    pub has_cover: bool,
    /// Whether the signed-in user likes the track, as their sync log says.
    pub liked: bool,
    /// A track's ISRC, when tagged.
    pub isrc: Option<String>,
}

/// The name a cover is cached under: Melete gives covers no version, and says
/// clients may keep one a day.
pub const COVER_TAG: &str = "cover";

impl Item {
    /// The item's length. `None` when the server doesn't know it.
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// The album id and tag of the cover that represents this item: an album's
    /// own, or a track's album's. Artists and genres have none.
    pub fn artwork_source(&self) -> Option<(&str, &str)> {
        match self.kind {
            ItemKind::Track | ItemKind::Album if self.has_cover => {
                self.album_id.as_deref().map(|album| (album, COVER_TAG))
            }
            _ => None,
        }
    }
}

/// A name with the id of the item it names, such as an artist credit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NameId {
    /// The named item's [`Item::id`].
    pub id: String,
    /// The name as shown.
    pub name: String,
}

/// One page of a list, and how many there are in all.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    /// This page's items, at most the query's `limit`.
    pub items: Vec<T>,
    /// How many items match across every page.
    pub total: u32,
    /// The 0-based index of this page's first item among them all.
    pub start_index: u32,
}

/// Melete's paged envelope, `{"items", "total"}`.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub(crate) struct WirePage<T> {
    pub items: Vec<T>,
    pub total: u32,
}

/// `MusicTrack`, trimmed to what the app shows.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireTrack {
    pub id: i64,
    pub title: String,
    pub album_id: i64,
    pub album: String,
    pub album_artist: String,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub duration_seconds: Option<f64>,
    #[serde(default, deserialize_with = "nullable")]
    pub genres: Vec<String>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub isrc: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub liked: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub artists: Vec<WireArtistRef>,
}

/// An artist credit on a track: `{"id", "name"}`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireArtistRef {
    pub id: i64,
    pub name: String,
}

/// `MusicAlbum`, trimmed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireAlbum {
    pub id: i64,
    pub title: String,
    pub artist_id: Option<i64>,
    pub artist: String,
    pub year: Option<i32>,
    pub tracks: i32,
    pub duration_seconds: Option<f64>,
    pub cover: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub genres: Vec<String>,
}

/// `MusicArtist`, trimmed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireArtist {
    pub id: i64,
    pub name: String,
    pub albums: i32,
}

/// `MusicGenre`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireGenre {
    pub name: String,
    pub tracks: i32,
}

/// A length in seconds, as Melete sends it, as a duration; a negative, infinite
/// or missing one is unknown.
fn seconds(value: Option<f64>) -> Option<Duration> {
    value
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

impl From<WireTrack> for Item {
    fn from(track: WireTrack) -> Self {
        let artist_items: Vec<NameId> = track
            .artists
            .into_iter()
            .map(|artist| NameId {
                id: artist.id.to_string(),
                name: artist.name,
            })
            .collect();
        Self {
            id: track.id.to_string(),
            name: track.title,
            kind: ItemKind::Track,
            album: Some(track.album),
            album_id: Some(track.album_id.to_string()),
            album_artist: Some(track.album_artist),
            album_artists: Vec::new(),
            artists: artist_items.iter().map(|a| a.name.clone()).collect(),
            artist_items,
            genres: track.genres,
            duration: seconds(track.duration_seconds),
            track_number: track.track_number,
            disc_number: track.disc_number,
            year: track.year,
            child_count: None,
            has_cover: true,
            liked: track.liked,
            isrc: track.isrc,
        }
    }
}

impl From<WireAlbum> for Item {
    fn from(album: WireAlbum) -> Self {
        let id = album.id.to_string();
        Self {
            album_artists: album
                .artist_id
                .map(|artist| NameId {
                    id: artist.to_string(),
                    name: album.artist.clone(),
                })
                .into_iter()
                .collect(),
            name: album.title,
            kind: ItemKind::Album,
            album_id: Some(id.clone()),
            id,
            album_artist: Some(album.artist),
            genres: album.genres,
            duration: seconds(album.duration_seconds),
            year: album.year,
            child_count: Some(album.tracks),
            has_cover: album.cover.is_some(),
            ..Self::default()
        }
    }
}

impl From<WireArtist> for Item {
    fn from(artist: WireArtist) -> Self {
        Self {
            id: artist.id.to_string(),
            name: artist.name,
            kind: ItemKind::Artist,
            child_count: Some(artist.albums),
            ..Self::default()
        }
    }
}

impl From<WireGenre> for Item {
    fn from(genre: WireGenre) -> Self {
        Self {
            id: genre.name.clone(),
            name: genre.name,
            kind: ItemKind::Genre,
            child_count: Some(genre.tracks),
            ..Self::default()
        }
    }
}

impl<W> WirePage<W> {
    /// The page as items, starting at `start_index`.
    pub(crate) fn into_page(self, start_index: u32) -> Page<Item>
    where
        Item: From<W>,
    {
        Page {
            items: self.items.into_iter().map(Item::from).collect(),
            total: self.total,
            start_index,
        }
    }
}

/// A missing field and an explicit `null` both mean "the default".
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(json: &str) -> Item {
        serde_json::from_str::<WireTrack>(json).unwrap().into()
    }

    #[test]
    fn a_track_from_the_server_becomes_an_item() {
        let page: WirePage<WireTrack> =
            serde_json::from_str(include_str!("../tests/fixtures/tracks.json")).unwrap();
        let page = page.into_page(0);
        assert_eq!(page.total, 4);
        let item = &page.items[0];
        assert_eq!(item.id, "4");
        assert_eq!(item.name, "Café Song");
        assert_eq!(item.kind, ItemKind::Track);
        assert_eq!(item.album.as_deref(), Some("Second Album"));
        assert_eq!(item.album_id.as_deref(), Some("1"));
        assert_eq!(item.album_artist.as_deref(), Some("Other Artist"));
        assert_eq!(item.artists, ["Other Artist"]);
        assert_eq!(
            item.artist_items,
            [NameId {
                id: "1".into(),
                name: "Other Artist".into()
            }]
        );
        assert_eq!(item.genres, ["Jazz"]);
        assert_eq!(item.duration(), Some(Duration::from_secs(22)));
        assert_eq!((item.disc_number, item.track_number), (Some(1), Some(1)));
        assert_eq!(item.year, Some(2020));
        assert!(!item.liked);
    }

    #[test]
    fn a_tracks_cover_is_its_albums() {
        let item = track(include_str!("../tests/fixtures/track.json"));
        assert_eq!(item.artwork_source(), Some(("2", COVER_TAG)));
    }

    #[test]
    fn an_album_without_a_cover_has_no_artwork() {
        let album: WireAlbum = serde_json::from_str(
            r#"{"id":1,"title":"A","artist_id":null,"artist":"Various Artists","year":null,
                "tracks":2,"duration_seconds":null,"cover":null}"#,
        )
        .unwrap();
        let item = Item::from(album);
        assert_eq!(item.artwork_source(), None);
        assert!(item.album_artists.is_empty(), "a compilation names no one");
        assert_eq!(item.duration(), None);
    }

    #[test]
    fn an_album_with_a_cover_and_an_artist() {
        let album: WireAlbum =
            serde_json::from_str(include_str!("../tests/fixtures/album.json")).unwrap();
        let item = Item::from(album);
        assert_eq!(item.kind, ItemKind::Album);
        assert_eq!(item.id, "2");
        assert_eq!(item.album_id.as_deref(), Some("2"));
        assert_eq!(item.child_count, Some(3));
        assert_eq!(item.artwork_source(), Some(("2", COVER_TAG)));
        assert_eq!(
            item.album_artists,
            [NameId {
                id: "2".into(),
                name: "Test Artist".into()
            }]
        );
    }

    #[test]
    fn a_genre_is_named_by_its_name() {
        let genre: WireGenre =
            serde_json::from_str(r#"{"name":"Rock","tracks":3,"albums":1}"#).unwrap();
        let item = Item::from(genre);
        assert_eq!((item.id.as_str(), item.name.as_str()), ("Rock", "Rock"));
        assert_eq!(item.kind, ItemKind::Genre);
    }

    #[test]
    fn a_bad_length_is_unknown() {
        assert_eq!(seconds(Some(-1.0)), None);
        assert_eq!(seconds(Some(f64::NAN)), None);
        assert_eq!(seconds(None), None);
        assert_eq!(seconds(Some(1.5)), Some(Duration::from_millis(1500)));
    }

    #[test]
    fn nulls_in_lists_decode_as_empty() {
        let item = track(
            r#"{"id":1,"title":"t","album_id":2,"album":"a","album_artist":"b",
                "track_number":null,"disc_number":null,"year":null,"duration_seconds":null,
                "genres":null,"codec":null,"bitrate_kbps":null,"sample_rate":null,
                "bit_depth":null,"channels":null,"isrc":null,"liked":null,"artists":null}"#,
        );
        assert!(item.genres.is_empty());
        assert!(item.artists.is_empty());
    }
}
