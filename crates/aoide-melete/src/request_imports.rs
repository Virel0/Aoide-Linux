//! Playlists asked for as songs (`/api/v1/requests/imports`): Melete matches
//! each track to a MusicBrainz recording and asks for it as this user, in the
//! background, a track at a time.

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::{MeleteClient, Result};

/// A playlist import, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlaylistImportSummary {
    /// The server's id for it.
    pub id: i64,
    /// The playlist's name.
    pub name: String,
    /// `queued`, `running`, `done` or `failed`.
    pub state: String,
    /// How it went, once done or failed.
    pub message: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Tracks in the playlist.
    pub tracks: u32,
    /// Tracks this import asked for.
    pub asked: u32,
    /// Tracks asked for already.
    pub already: u32,
    /// Tracks no MusicBrainz recording fitted.
    pub unmatched: u32,
    /// Of those asked for, the ones still waiting for an admin.
    pub waiting: u32,
}

/// One track of an import.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlaylistImportTrack {
    /// Its place in the playlist, from 1.
    pub position: u32,
    /// Its title.
    pub title: String,
    /// As the playlist names them.
    pub artist: Option<String>,
    /// `pending`, `asked`, `already` or `unmatched`.
    pub state: String,
    /// Why it wasn't asked for, when unmatched.
    pub why: Option<String>,
    /// The request it became, or the one asked already.
    pub request_id: Option<i64>,
}

/// A playlist import, track by track.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlaylistImport {
    /// What the list shows of it.
    #[serde(flatten)]
    pub summary: PlaylistImportSummary,
    /// Its tracks, in order.
    pub items: Vec<PlaylistImportTrack>,
}

impl MeleteClient {
    /// Asks for every track of a playlist as a song, from `csv` with a column of
    /// track names (and artist, album, duration and ISRC when there are).
    ///
    /// # Errors
    ///
    /// HTTP 400 as [`Error::Http`](crate::Error::Http) for a file that isn't a
    /// playlist; 409 when MusicBrainz is off; 413 past 8 MB.
    pub async fn import_playlist(&self, name: &str, csv: &str) -> Result<PlaylistImportSummary> {
        let body = json!({ "name": name, "csv": csv });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "requests", "imports"],
            &[],
            Some(&body),
        )
        .await
    }

    /// This user's playlist imports, newest first (100 at most).
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn playlist_imports(&self) -> Result<Vec<PlaylistImportSummary>> {
        self.get(&["api", "v1", "requests", "imports"], &[]).await
    }

    /// One playlist import, track by track.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) once it's been removed.
    pub async fn playlist_import(&self, id: i64) -> Result<PlaylistImport> {
        let id = id.to_string();
        self.get(&["api", "v1", "requests", "imports", &id], &[])
            .await
    }

    /// Removes a playlist import, taking back its requests still waiting for an
    /// admin (approved ones stay: they're on their way).
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn remove_playlist_import(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(
            Method::DELETE,
            &["api", "v1", "requests", "imports", &id],
            &[],
        )
        .await
    }
}

/// A playlist as the CSV Melete reads, in Exportify's columns: a header, then
/// one row a track (title, artists, album, length in milliseconds, ISRC).
pub fn playlist_csv<'a>(
    tracks: impl IntoIterator<
        Item = (
            &'a str,
            &'a str,
            Option<&'a str>,
            Option<u64>,
            Option<&'a str>,
        ),
    >,
) -> String {
    fn quoted(field: &str) -> String {
        format!("\"{}\"", field.replace('"', "\"\""))
    }
    let mut csv = String::from("Track Name,Artist Name(s),Album Name,Track Duration (ms),ISRC\n");
    for (title, artist, album, duration_ms, isrc) in tracks {
        let row = [
            quoted(title),
            quoted(artist),
            quoted(album.unwrap_or_default()),
            duration_ms.map(|ms| ms.to_string()).unwrap_or_default(),
            quoted(isrc.unwrap_or_default()),
        ];
        csv.push_str(&row.join(","));
        csv.push('\n');
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_becomes_exportify_csv_with_quotes_doubled() {
        let csv = playlist_csv([
            (
                "Say \"Hi\"",
                "Mara Vey, Odd Hours",
                Some("Low Tide"),
                Some(201_000),
                None,
            ),
            ("Plain", "Someone", None, None, Some("USX9P0000001")),
        ]);
        assert_eq!(
            csv,
            "Track Name,Artist Name(s),Album Name,Track Duration (ms),ISRC\n\
             \"Say \"\"Hi\"\"\",\"Mara Vey, Odd Hours\",\"Low Tide\",201000,\"\"\n\
             \"Plain\",\"Someone\",\"\",,\"USX9P0000001\"\n"
        );
    }
}
