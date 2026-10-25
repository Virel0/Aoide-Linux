use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};

use crate::ticks;

/// The kinds of item a music client meets. Anything else Jellyfin serves (movies,
/// episodes, books) decodes as `Unknown` rather than failing the whole page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
pub enum ItemKind {
    Audio,
    MusicAlbum,
    MusicArtist,
    MusicGenre,
    Playlist,
    /// A library. Its `collection_type` says what kind.
    CollectionFolder,
    /// A folder on disk, met when browsing by folder.
    Folder,
    #[default]
    #[serde(other)]
    Unknown,
}

/// A trimmed `BaseItemDto`: what a music client renders, nothing more. Every field
/// but the id may be missing or null.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(rename = "Type", default, deserialize_with = "nullable")]
    pub kind: ItemKind,
    /// Libraries, folders, albums and playlists are folders; tracks are not.
    #[serde(default, deserialize_with = "nullable")]
    pub is_folder: bool,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub album_artist: Option<String>,
    /// The album artists with their ids.
    #[serde(default, deserialize_with = "nullable")]
    pub album_artists: Vec<NameId>,
    /// Artist names, in credit order.
    #[serde(default, deserialize_with = "nullable")]
    pub artists: Vec<String>,
    /// The same artists with their ids.
    #[serde(default, deserialize_with = "nullable")]
    pub artist_items: Vec<NameId>,
    /// Genres in the order Jellyfin returns them, which is the file's tag order.
    #[serde(default, deserialize_with = "nullable")]
    pub genres: Vec<String>,
    /// Duration in ticks; see [`Item::duration`].
    pub run_time_ticks: Option<i64>,
    /// Track number within its disc.
    pub index_number: Option<i32>,
    /// Disc number within its album.
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub parent_id: Option<String>,
    pub child_count: Option<i32>,
    /// On libraries only: `"music"` marks a music library.
    pub collection_type: Option<String>,
    /// Image type (`"Primary"`, …) to tag. A tag changes when the image does.
    #[serde(default, deserialize_with = "nullable")]
    pub image_tags: HashMap<String, String>,
    /// A track's borrowed album cover.
    pub album_primary_image_tag: Option<String>,
    pub user_data: Option<UserData>,
}

impl ItemKind {
    /// The server's name for the type, as `includeItemTypes` takes it. `None` for
    /// [`ItemKind::Unknown`].
    pub fn type_name(self) -> Option<&'static str> {
        Some(match self {
            ItemKind::Audio => "Audio",
            ItemKind::MusicAlbum => "MusicAlbum",
            ItemKind::MusicArtist => "MusicArtist",
            ItemKind::MusicGenre => "MusicGenre",
            ItemKind::Playlist => "Playlist",
            ItemKind::CollectionFolder => "CollectionFolder",
            ItemKind::Folder => "Folder",
            ItemKind::Unknown => return None,
        })
    }
}

impl Item {
    pub fn duration(&self) -> Option<Duration> {
        self.run_time_ticks.map(ticks::to_duration)
    }

    /// The item id and tag of the image that represents this item: its own primary
    /// image, or else its album's (tracks usually have no cover of their own).
    pub fn artwork_source(&self) -> Option<(&str, &str)> {
        if let Some(tag) = self.image_tags.get("Primary") {
            return Some((&self.id, tag));
        }
        match (&self.album_id, &self.album_primary_image_tag) {
            (Some(album_id), Some(tag)) => Some((album_id, tag)),
            _ => None,
        }
    }
}

/// A name with the id of the item it names, such as an artist credit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NameId {
    pub id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
}

/// The signed-in user's state for an item.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    #[serde(default, deserialize_with = "nullable")]
    pub is_favorite: bool,
    /// Jellyfin counts a play when playback starts, so this is "times started".
    #[serde(default, deserialize_with = "nullable")]
    pub play_count: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub played: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub playback_position_ticks: i64,
    /// `None` when never played, or when the server's date couldn't be read.
    #[serde(rename = "LastPlayedDate", default, deserialize_with = "date")]
    pub last_played: Option<SystemTime>,
}

/// Jellyfin's paged envelope: one page of `items`, and how many there are in all.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase", bound(deserialize = "T: Deserialize<'de>"))]
pub struct Page<T> {
    #[serde(default, deserialize_with = "nullable")]
    pub items: Vec<T>,
    #[serde(default, deserialize_with = "nullable")]
    pub total_record_count: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub start_index: u32,
}

/// A missing field and an explicit `null` both mean "the default".
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn date<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<SystemTime>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|raw| parse_date(&raw)))
}

/// Parses the ISO 8601 dates Jellyfin emits: with or without fractional seconds (up
/// to seven digits from .NET, more are accepted), with `Z`, an offset, or no zone at
/// all (read as UTC). `None` for anything else.
pub(crate) fn parse_date(raw: &str) -> Option<SystemTime> {
    let (date, time) = raw.trim().split_once(['T', ' '])?;

    let mut parts = date.splitn(3, '-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let zone_at = time.find(['Z', 'z', '+', '-']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(zone_at);
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = hms.splitn(3, ':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let second: i64 = parts.next().unwrap_or("0").parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 || hour < 0 || minute < 0 || second < 0 {
        return None;
    }

    let nanos = if fraction.is_empty() {
        0
    } else {
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits = &fraction[..fraction.len().min(9)];
        digits.parse::<u32>().ok()? * 10u32.pow(9 - digits.len() as u32)
    };

    let offset = match zone {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let body = zone.get(1..)?;
            let (h, m) = match body.split_once(':') {
                Some(pair) => pair,
                None if body.len() == 4 => (body.get(..2)?, body.get(2..)?),
                None => (body, "0"),
            };
            let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
            sign * (h * 3600 + m * 60)
        }
    };

    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let whole = Duration::from_secs(seconds.unsigned_abs());
    let base = if seconds >= 0 {
        UNIX_EPOCH.checked_add(whole)?
    } else {
        UNIX_EPOCH.checked_sub(whole)?
    };
    base.checked_add(Duration::from_nanos(u64::from(nanos)))
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64, nanos: u32) -> SystemTime {
        UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    #[test]
    fn decodes_a_page_of_album_tracks() {
        let page: Page<Item> =
            serde_json::from_str(include_str!("../tests/fixtures/album_tracks.json")).unwrap();
        assert_eq!(page.total_record_count, 3);
        assert_eq!(page.items.len(), 3);

        let first = &page.items[0];
        assert_eq!(first.id, "a1b2c3d4e5f60718293a4b5c6d7e8f90");
        assert_eq!(first.name, "Harbour Lights");
        assert_eq!(first.kind, ItemKind::Audio);
        assert_eq!(first.album.as_deref(), Some("Low Tide Radio"));
        assert_eq!(
            first.album_id.as_deref(),
            Some("b0a1c2d3e4f5061728394a5b6c7d8e9f")
        );
        assert_eq!(first.album_artist.as_deref(), Some("The Quiet Engines"));
        assert_eq!(first.artists, ["The Quiet Engines", "Mara Vey"]);
        assert_eq!(
            first
                .artist_items
                .iter()
                .map(|a| (a.id.as_str(), a.name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("c0ffee00c0ffee00c0ffee00c0ffee00", "The Quiet Engines"),
                ("decade00decade00decade00decade00", "Mara Vey"),
            ]
        );
        assert_eq!(first.album_artists[0].name, "The Quiet Engines");
        assert_eq!(first.index_number, Some(1));
        assert_eq!(first.parent_index_number, Some(1));
        assert_eq!(first.production_year, Some(2021));
        assert_eq!(first.duration(), Some(Duration::from_millis(243_500)));
        assert_eq!(first.image_tags.get("Primary"), None);
        assert_eq!(
            first.artwork_source(),
            Some((
                "b0a1c2d3e4f5061728394a5b6c7d8e9f",
                "7d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a"
            ))
        );

        let user = first.user_data.as_ref().unwrap();
        assert!(user.is_favorite);
        assert!(user.played);
        assert_eq!(user.play_count, 12);
        // 2026-03-14T21:07:45.1234567Z
        assert_eq!(user.last_played, Some(at(1_773_522_465, 123_456_700)));
    }

    #[test]
    fn genres_keep_the_servers_order() {
        let page: Page<Item> =
            serde_json::from_str(include_str!("../tests/fixtures/album_tracks.json")).unwrap();
        assert_eq!(
            page.items[0].genres,
            ["Shoegaze", "Dream Pop", "Indie Rock"]
        );
        assert_eq!(page.items[1].genres, ["Dream Pop", "Shoegaze"]);
    }

    #[test]
    fn a_never_played_track_has_no_last_played_date() {
        let page: Page<Item> =
            serde_json::from_str(include_str!("../tests/fixtures/album_tracks.json")).unwrap();
        let third = page.items[2].user_data.as_ref().unwrap();
        assert_eq!(third.play_count, 0);
        assert_eq!(third.last_played, None);
        assert!(!third.is_favorite);
    }

    #[test]
    fn decodes_library_views_with_nulls_and_unmodelled_types() {
        let page: Page<Item> =
            serde_json::from_str(include_str!("../tests/fixtures/user_views.json")).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].kind, ItemKind::CollectionFolder);
        assert_eq!(page.items[0].collection_type.as_deref(), Some("music"));
        assert_eq!(page.items[1].collection_type.as_deref(), Some("movies"));
    }

    #[test]
    fn an_unmodelled_type_is_unknown_rather_than_an_error() {
        let item: Item =
            serde_json::from_str(r#"{"Id":"x","Name":"A Film","Type":"Movie"}"#).unwrap();
        assert_eq!(item.kind, ItemKind::Unknown);
    }

    #[test]
    fn a_sparse_item_decodes_with_defaults() {
        let item: Item = serde_json::from_str(r#"{"Id":"x"}"#).unwrap();
        assert!(item.name.is_empty());
        assert_eq!(item.kind, ItemKind::Unknown);
        assert!(item.artists.is_empty());
        assert_eq!(item.duration(), None);
        assert_eq!(item.user_data, None);
        assert_eq!(item.artwork_source(), None);
    }

    #[test]
    fn explicit_nulls_decode_like_missing_fields() {
        let item: Item = serde_json::from_str(
            r#"{"Id":"x","Name":null,"Type":null,"Artists":null,"Genres":null,
                "ImageTags":null,"Album":null,"UserData":{"IsFavorite":null,"PlayCount":null}}"#,
        )
        .unwrap();
        assert!(item.name.is_empty());
        assert!(item.genres.is_empty());
        assert_eq!(item.user_data, Some(UserData::default()));
    }

    #[test]
    fn an_item_without_an_id_is_an_error() {
        assert!(serde_json::from_str::<Item>(r#"{"Name":"x"}"#).is_err());
    }

    #[test]
    fn an_items_own_cover_wins_over_its_albums() {
        let item: Item = serde_json::from_str(
            r#"{"Id":"i","ImageTags":{"Primary":"own"},"AlbumId":"a","AlbumPrimaryImageTag":"alb"}"#,
        )
        .unwrap();
        assert_eq!(item.artwork_source(), Some(("i", "own")));
    }

    #[test]
    fn a_page_without_its_counts_still_decodes() {
        let page: Page<Item> = serde_json::from_str(r#"{"Items":[{"Id":"x"}]}"#).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total_record_count, 0);
    }

    #[test]
    fn dates_parse_in_every_shape_jellyfin_sends() {
        assert_eq!(parse_date("1970-01-01T00:00:00Z"), Some(UNIX_EPOCH));
        assert_eq!(
            parse_date("2024-05-01T12:34:56.1234567Z"),
            Some(at(1_714_566_896, 123_456_700))
        );
        assert_eq!(
            parse_date("2024-05-01T12:34:56Z"),
            Some(at(1_714_566_896, 0))
        );
        assert_eq!(
            parse_date("2024-05-01T12:34:56.5"),
            Some(at(1_714_566_896, 500_000_000))
        );
        assert_eq!(
            parse_date("2024-05-01T14:34:56+02:00"),
            Some(at(1_714_566_896, 0))
        );
        assert_eq!(
            parse_date("2024-05-01T07:34:56-0500"),
            Some(at(1_714_566_896, 0))
        );
        assert_eq!(parse_date("2000-02-29T00:00:00Z"), Some(at(951_782_400, 0)));
    }

    #[test]
    fn dates_before_the_epoch_parse() {
        assert_eq!(
            parse_date("1969-12-31T23:59:59Z"),
            UNIX_EPOCH.checked_sub(Duration::from_secs(1))
        );
    }

    #[test]
    fn garbage_dates_are_none() {
        for raw in [
            "",
            "yesterday",
            "2024-13-01T00:00:00Z",
            "2024-05-01",
            "2024-05-01T25:00:00Z",
            "2024-05-01T12:00:00.12x4Z",
        ] {
            assert_eq!(parse_date(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn an_unreadable_last_played_date_doesnt_fail_the_item() {
        let data: UserData =
            serde_json::from_str(r#"{"PlayCount":3,"LastPlayedDate":"not a date"}"#).unwrap();
        assert_eq!(data.play_count, 3);
        assert_eq!(data.last_played, None);
    }
}
