//! Melete's answers as the tests' stand-in servers send them.

use serde_json::{Value, json};

/// A `MusicTrack` with `id` and `title`, the rest plain.
pub fn track(id: i64, title: &str) -> Value {
    json!({
        "id": id, "title": title, "artist": "An Artist", "album_id": 1, "album": "An Album",
        "album_artist": "An Artist", "track_number": 1, "disc_number": 1, "year": 2020,
        "duration_seconds": 200.0, "genres": [], "codec": "flac", "bitrate_kbps": 900,
        "sample_rate": 44100, "bit_depth": 16, "channels": 2, "spatial": null, "isrc": null,
        "mb_recording_id": null, "mb_track_id": null, "mb_release_id": null,
        "mb_release_group_id": null, "mb_artist_id": null, "gapless": null, "liked": false,
        "artists": [{"id": 1, "name": "An Artist", "mb_artist_id": null}],
    })
}

/// A page of `items`, all of them.
pub fn page(items: Vec<Value>) -> Value {
    json!({ "total": items.len(), "items": items })
}
