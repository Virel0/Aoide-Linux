//! From a Jellyfin item to something the player can play.
//!
//! The stream URL comes from aoide-jellyfin (the token as `ApiKey`); where the
//! sidecar has measured a track, its sound bounds (Skip silence) and loudness
//! gain (Level volume) go with it. An unmeasured track plays plainly.

use std::collections::HashMap;

use aoide_audio::{MediaSource, MixTrack, PlayerTrack, SoundBounds, loudness};
use aoide_jellyfin::{Item, JellyfinClient, StreamQuality, Url};
use aoide_sidecar::{AudioAnalysis, AudioAnalysisResponse, Measurement, SoundBoundsResponse};
use aoide_store::{ContentKey, Millis, TrackLink, TrackRef};

use crate::config::Settings;

/// What the app knows about a queued or listed track, for drawing it and for
/// the store. Built from an [`Item`]; cheap to clone.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMeta {
    pub id: String,
    pub title: String,
    /// The credited artists, joined for display.
    pub artist: String,
    /// The album artist where there is one: what the history and the taste
    /// ranking key on, as the phone does.
    pub album_artist: Option<String>,
    pub album: String,
    pub album_id: Option<String>,
    /// The album artist's id (else the first credited artist's), for Go to Artist.
    pub artist_id: Option<String>,
    /// The item and tag of the cover to draw.
    pub artwork: Option<(String, String)>,
    pub duration_ms: Option<i64>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub favourite: bool,
}

impl TrackMeta {
    pub fn from_item(item: &Item) -> Self {
        let artist = if item.artists.is_empty() {
            item.album_artist.clone().unwrap_or_default()
        } else {
            item.artists.join(", ")
        };
        Self {
            id: item.id.clone(),
            title: item.name.clone(),
            artist,
            album_artist: item.album_artist.clone().filter(|a| !a.is_empty()),
            album: item.album.clone().unwrap_or_default(),
            album_id: item.album_id.clone(),
            artist_id: item
                .album_artists
                .first()
                .or_else(|| item.artist_items.first())
                .map(|a| a.id.clone()),
            artwork: item
                .artwork_source()
                .map(|(id, tag)| (id.to_owned(), tag.to_owned())),
            duration_ms: item
                .duration()
                .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
            track_number: item.index_number,
            disc_number: item.parent_index_number,
            year: item.production_year,
            genres: item.genres.clone(),
            favourite: item.user_data.as_ref().is_some_and(|u| u.is_favorite),
        }
    }

    /// The artist the history keys on: the album artist, else the first credit.
    pub fn history_artist(&self) -> String {
        self.album_artist
            .clone()
            .or_else(|| self.artist.split(", ").next().map(str::to_owned))
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "Unknown Artist".to_owned())
    }

    /// The fingerprint that survives a rescan.
    pub fn content_key(&self) -> String {
        ContentKey::make(
            &self.history_artist(),
            &self.album,
            &self.title,
            self.duration_ms,
        )
    }

    pub fn link(&self) -> TrackLink {
        TrackLink::new(self.id.clone(), self.content_key())
    }

    /// The row for the store's track cache.
    pub fn track_ref(&self, now: Millis) -> TrackRef {
        let mut row = TrackRef::new(
            self.id.clone(),
            self.content_key(),
            self.title.clone(),
            self.history_artist(),
            self.album.clone(),
            now,
        )
        .with_genres(&self.genres);
        row.album_artist = self.album_artist.clone();
        row.album_id = self.album_id.clone();
        row.duration_ms = self.duration_ms;
        row.year = self.year.map(i64::from);
        row
    }
}

/// What the sidecar has said about tracks, kept for the session so a track
/// played twice is asked about once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measures {
    bounds: HashMap<String, Option<SoundBounds>>,
    analysis: HashMap<String, Option<AudioAnalysis>>,
}

impl Measures {
    /// Ids not yet answered for, in order, without repeats.
    pub fn unknown<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        ids.into_iter()
            .filter(|id| !self.bounds.contains_key(*id) || !self.analysis.contains_key(*id))
            .filter(|id| seen.insert(*id))
            .map(str::to_owned)
            .collect()
    }

    /// Takes in one round of answers. A pending or unknown track is left out, so
    /// it is asked about again next time.
    pub fn absorb(
        &mut self,
        ids: &[String],
        bounds: Option<&SoundBoundsResponse>,
        analysis: Option<&AudioAnalysisResponse>,
    ) {
        for id in ids {
            if let Some(bounds) = bounds {
                match bounds.get(id) {
                    Measurement::Measured(b) => {
                        self.bounds.insert(id.clone(), sound_bounds(b));
                    }
                    Measurement::NothingToReport => {
                        self.bounds.insert(id.clone(), None);
                    }
                    Measurement::Pending | Measurement::Unknown => {}
                }
            }
            if let Some(analysis) = analysis {
                match analysis.get(id) {
                    Measurement::Measured(a) => {
                        self.analysis.insert(id.clone(), Some(*a));
                    }
                    Measurement::NothingToReport => {
                        self.analysis.insert(id.clone(), None);
                    }
                    Measurement::Pending | Measurement::Unknown => {}
                }
            }
        }
    }

    /// Records a measurement directly.
    #[cfg(test)]
    pub fn insert(
        &mut self,
        id: &str,
        bounds: Option<SoundBounds>,
        analysis: Option<AudioAnalysis>,
    ) {
        self.bounds.insert(id.to_owned(), bounds);
        self.analysis.insert(id.to_owned(), analysis);
    }

    pub fn bounds(&self, id: &str) -> Option<SoundBounds> {
        self.bounds.get(id).copied().flatten()
    }

    pub fn analysis(&self, id: &str) -> Option<AudioAnalysis> {
        self.analysis.get(id).copied().flatten()
    }
}

/// A span the engine can play: starting before it ends, and never negative.
fn sound_bounds(bounds: &aoide_sidecar::SoundBounds) -> Option<SoundBounds> {
    let start = u64::try_from(bounds.sound_start_ms).ok()?;
    let end = u64::try_from(bounds.sound_end_ms).ok()?;
    (end > start).then(|| SoundBounds::new(start, end))
}

/// The sound bounds and loudness gain a track plays with: what the sidecar
/// measured, as far as the settings ask for it.
pub fn applied(
    id: &str,
    measures: &Measures,
    settings: &Settings,
) -> (Option<SoundBounds>, Option<f64>) {
    let sound = settings.skip_silence.then(|| measures.bounds(id)).flatten();
    // A measured track the sidecar leaves alone is 0 dB, not `None`: its
    // measurement wins over the file's tags, "leave it" included.
    let gain_db = if settings.level_volume {
        measures
            .analysis(id)
            .filter(|a| a.loudness_lufs.is_some_and(f64::is_finite))
            .map(|a| loudness::gain_db(a.loudness_lufs, a.true_peak_dbfs).unwrap_or(0.0))
    } else {
        None
    };
    (sound, gain_db)
}

/// One playable track and the URL it streams from (kept for tests: the
/// player's source does not give it back).
#[derive(Debug, Clone)]
pub struct BuiltTrack {
    pub meta: TrackMeta,
    #[allow(dead_code)]
    pub url: Url,
    pub track: PlayerTrack,
}

/// Builds the player's track for `meta`: the stream, and the measurements the
/// settings ask for.
pub fn build(
    client: &JellyfinClient,
    meta: &TrackMeta,
    measures: &Measures,
    settings: &Settings,
) -> Result<BuiltTrack, aoide_jellyfin::Error> {
    let url = client.stream_url(&meta.id, StreamQuality::Lossless)?;
    let analysis = measures.analysis(&meta.id);
    let (sound, gain_db) = applied(&meta.id, measures, settings);
    let mix = MixTrack {
        album: meta.album.clone(),
        track_number: meta.track_number.map(i64::from),
        duration_seconds: meta.duration_ms.map(|ms| ms as f64 / 1000.0),
        sound,
        bpm: analysis.and_then(|a| a.bpm),
        bpm_stability: analysis.and_then(|a| a.bpm_stability),
    };
    Ok(BuiltTrack {
        meta: meta.clone(),
        track: PlayerTrack {
            // Made unique when the track is queued.
            key: meta.id.clone(),
            source: MediaSource::http(url.as_str()),
            mix,
            gain_db,
        },
        url,
    })
}

/// The engine's view of a built track, as the player hands it over.
#[cfg(test)]
pub fn spec(id: aoide_audio::EntryId, built: &BuiltTrack) -> aoide_audio::TrackSpec {
    aoide_audio::TrackSpec::new(id, built.track.source.clone())
        .with_bounds(built.track.mix.sound)
        .with_gain_db(built.track.gain_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};

    fn client() -> JellyfinClient {
        JellyfinClient::new(
            ServerAddress::parse("https://music.example.net").unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "user1".into(),
            access_token: "secret-token".into(),
            username: "alice".into(),
        })
    }

    fn item(json: &str) -> Item {
        serde_json::from_str(json).unwrap()
    }

    fn meta() -> TrackMeta {
        TrackMeta::from_item(&item(
            r#"{"Id":"t1","Name":"Harbour Lights","Type":"Audio","Album":"Low Tide",
                "AlbumId":"a1","AlbumArtist":"The Quiet Engines",
                "Artists":["The Quiet Engines","Mara Vey"],"RunTimeTicks":2435000000,
                "IndexNumber":3,"ParentIndexNumber":1,"Genres":["Ambient"],
                "AlbumPrimaryImageTag":"tag1","UserData":{"IsFavorite":true}}"#,
        ))
    }

    fn analysis(lufs: f64, peak: f64) -> AudioAnalysis {
        AudioAnalysis {
            loudness_lufs: Some(lufs),
            true_peak_dbfs: Some(peak),
            bpm: Some(120.0),
            bpm_confidence: Some(0.9),
            bpm_stability: Some(0.8),
        }
    }

    #[test]
    fn meta_reads_what_a_row_needs() {
        let meta = meta();
        assert_eq!(meta.title, "Harbour Lights");
        assert_eq!(meta.artist, "The Quiet Engines, Mara Vey");
        assert_eq!(meta.history_artist(), "The Quiet Engines");
        assert_eq!(meta.duration_ms, Some(243_500));
        assert_eq!(meta.artwork, Some(("a1".into(), "tag1".into())));
        assert!(meta.favourite);
        assert_eq!(meta.track_ref(5).genre_list(), ["Ambient"]);
    }

    #[test]
    fn the_stream_url_carries_the_token_as_apikey() {
        let built = build(
            &client(),
            &meta(),
            &Measures::default(),
            &Settings::default(),
        )
        .unwrap();
        let pairs: Vec<(String, String)> = built
            .url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(built.url.path(), "/Audio/t1/universal");
        assert!(
            pairs.contains(&("ApiKey".into(), "secret-token".into())),
            "{pairs:?}"
        );
        assert!(!pairs.iter().any(|(k, _)| k == "api_key"));
    }

    #[test]
    fn an_unmeasured_track_plays_plainly() {
        let built = build(
            &client(),
            &meta(),
            &Measures::default(),
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(built.track.mix.sound, None);
        assert_eq!(built.track.gain_db, None);
        assert_eq!(built.track.mix.album, "Low Tide");
        assert_eq!(built.track.mix.track_number, Some(3));
        assert_eq!(built.track.mix.duration_seconds, Some(243.5));
        let spec = spec(aoide_audio::EntryId(7), &built);
        assert_eq!(spec.bounds, None);
        assert_eq!(spec.gain_db, None);
    }

    #[test]
    fn bounds_and_gain_are_applied_when_measured() {
        let mut measures = Measures::default();
        measures.insert(
            "t1",
            Some(SoundBounds::new(1_200, 240_000)),
            Some(analysis(-12.0, -1.0)),
        );
        let built = build(&client(), &meta(), &measures, &Settings::default()).unwrap();
        let spec = spec(aoide_audio::EntryId(7), &built);
        assert_eq!(spec.bounds, Some(SoundBounds::new(1_200, 240_000)));
        assert_eq!(spec.gain_db, Some(-6.0));
        assert_eq!(built.track.mix.bpm, Some(120.0));
        assert_eq!(built.track.mix.bpm_stability, Some(0.8));
    }

    #[test]
    fn the_settings_turn_each_measurement_off() {
        let mut measures = Measures::default();
        measures.insert(
            "t1",
            Some(SoundBounds::new(1_200, 240_000)),
            Some(analysis(-12.0, -1.0)),
        );
        let settings = Settings {
            skip_silence: false,
            level_volume: false,
            ..Settings::default()
        };
        let built = build(&client(), &meta(), &measures, &settings).unwrap();
        assert_eq!(built.track.mix.sound, None);
        assert_eq!(built.track.gain_db, None);
    }

    #[test]
    fn a_quiet_track_is_not_turned_up() {
        let mut measures = Measures::default();
        measures.insert("t1", None, Some(analysis(-23.0, -6.0)));
        let built = build(&client(), &meta(), &measures, &Settings::default()).unwrap();
        // Measured and left alone: 0 dB, which also keeps the file's own
        // ReplayGain from turning it up.
        assert_eq!(built.track.gain_db, Some(0.0));
    }

    #[test]
    fn sidecar_answers_are_absorbed_and_pending_ones_asked_again() {
        let bounds: SoundBoundsResponse = serde_json::from_str(
            r#"{"bounds":{"t1":{"soundStartMs":100,"soundEndMs":9000},"t2":null,
                "t3":{"soundStartMs":500,"soundEndMs":400}},"pending":["t4"]}"#,
        )
        .unwrap();
        let analysis: AudioAnalysisResponse = serde_json::from_str(
            r#"{"analysis":{"t1":{"loudnessLufs":-9.0},"t2":null,"t3":null},"pending":["t4"]}"#,
        )
        .unwrap();
        let ids: Vec<String> = ["t1", "t2", "t3", "t4"].map(String::from).to_vec();
        let mut measures = Measures::default();
        measures.absorb(&ids, Some(&bounds), Some(&analysis));
        assert_eq!(measures.bounds("t1"), Some(SoundBounds::new(100, 9_000)));
        assert_eq!(measures.bounds("t2"), None);
        // An end before the start is no span at all.
        assert_eq!(measures.bounds("t3"), None);
        assert_eq!(measures.analysis("t1").unwrap().loudness_lufs, Some(-9.0));
        assert_eq!(
            measures.unknown(["t1", "t2", "t3", "t4", "t5", "t4"]),
            ["t4", "t5"]
        );
    }

    #[test]
    fn content_keys_match_the_stores_rule() {
        let meta = meta();
        assert_eq!(
            meta.content_key(),
            ContentKey::make(
                "The Quiet Engines",
                "Low Tide",
                "Harbour Lights",
                Some(243_500)
            )
        );
        assert_eq!(meta.link().jellyfin_id, "t1");
    }
}
