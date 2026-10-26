//! The local mirror of the library, and the fingerprint that survives a rescan.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::Millis;
use crate::record::{Record, int, opt_int, opt_text, text};

/// A row of the local mirror of the library. Never synced: every device rebuilds it
/// from its own connection to the same server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackRef {
    /// The id Jellyfin gives the track today; the key.
    pub jellyfin_id: String,
    /// The [`ContentKey`] fingerprint, which outlives the id.
    pub content_key: String,
    /// The recording's MusicBrainz id, when the file is tagged with one.
    pub musicbrainz_id: Option<String>,
    /// The track's title.
    pub title: String,
    /// The track's artist, as Jellyfin reports it.
    pub artist: String,
    /// The album's title.
    pub album: String,
    /// The album's artist, when tagged.
    pub album_artist: Option<String>,
    /// The album's own id, for drawing a playlist cover from what it holds.
    pub album_id: Option<String>,
    /// The track's length in milliseconds, when Jellyfin knows it.
    pub duration_ms: Option<i64>,
    /// The release year, when tagged.
    pub year: Option<i64>,
    /// A JSON array. Text because SQLite has no array type and it is only read whole.
    pub genres: String,
    /// When a library scan last saw the track, in milliseconds since the epoch.
    pub last_seen_at: Millis,
}

impl TrackRef {
    /// A track with no genres and the optional fields empty.
    pub fn new(
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        last_seen_at: Millis,
    ) -> Self {
        Self {
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            genres: "[]".to_owned(),
            last_seen_at,
            ..Self::default()
        }
    }

    /// The same track with `genres` set; see [`Self::set_genres`].
    pub fn with_genres<S: AsRef<str>>(mut self, genres: &[S]) -> Self {
        self.set_genres(genres);
        self
    }

    /// Stores `genres` as the JSON array the `genres` column holds.
    pub fn set_genres<S: AsRef<str>>(&mut self, genres: &[S]) {
        let list: Vec<&str> = genres.iter().map(AsRef::as_ref).collect();
        self.genres = serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_owned());
    }

    /// The genres, read back from the JSON array; empty if it does not parse.
    pub fn genre_list(&self) -> Vec<String> {
        serde_json::from_str(&self.genres).unwrap_or_default()
    }
}

impl Record for TrackRef {
    const TABLE: &'static str = "track";
    const KEY: &'static str = "jellyfinId";
    const COLUMNS: &'static [&'static str] = &[
        "jellyfinId",
        "contentKey",
        "musicbrainzId",
        "title",
        "artist",
        "album",
        "albumArtist",
        "albumId",
        "durationMs",
        "year",
        "genres",
        "lastSeenAt",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            jellyfin_id: row.get("jellyfinId")?,
            content_key: row.get("contentKey")?,
            musicbrainz_id: row.get("musicbrainzId")?,
            title: row.get("title")?,
            artist: row.get("artist")?,
            album: row.get("album")?,
            album_artist: row.get("albumArtist")?,
            album_id: row.get("albumId")?,
            duration_ms: row.get("durationMs")?,
            year: row.get("year")?,
            genres: row.get("genres")?,
            last_seen_at: row.get("lastSeenAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.jellyfin_id),
            text(&self.content_key),
            opt_text(&self.musicbrainz_id),
            text(&self.title),
            text(&self.artist),
            text(&self.album),
            opt_text(&self.album_artist),
            opt_text(&self.album_id),
            opt_int(self.duration_ms),
            opt_int(self.year),
            text(&self.genres),
            int(self.last_seen_at),
        ]
    }

    fn key(&self) -> &str {
        &self.jellyfin_id
    }
}

/// The fingerprint that survives a Jellyfin rescan.
///
/// Normalised hard, because the point is to match the *same recording* after its id
/// changed. Duration is rounded to the second so a re-encode that shifts it by a few
/// milliseconds still matches.
pub struct ContentKey;

impl ContentKey {
    /// The fingerprint of a recording: artist, album and title folded, and the duration
    /// rounded to the second, joined with `|`.
    pub fn make(artist: &str, album: &str, title: &str, duration_ms: Option<i64>) -> String {
        let seconds = duration_ms.map_or(0, |ms| (ms + 500) / 1000);
        [
            normalise(artist),
            normalise(album),
            normalise(title),
            seconds.to_string(),
        ]
        .join("|")
    }
}

/// Diacritic- and case-insensitive folding, then trimmed, then one pass collapsing
/// double spaces — the phone's order of operations.
fn normalise(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.trim().replace("  ", " ")
}
