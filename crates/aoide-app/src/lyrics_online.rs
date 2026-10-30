//! Lyrics from the internet, for a song the server has none for: LRCLIB,
//! through aoide-lyrics' `LyricsFinder`. On by default, with a switch in
//! Settings → Lyrics.
//!
//! What LRCLIB says is kept on disk per track for 30 days, as its terms ask
//! of clients, "nothing found" included, so a song is asked about at most
//! once a month. A lookup where LRCLIB didn't answer at all is not kept:
//! that says nothing about the song.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aoide_lyrics::{LyricsFinder, TrackQuery};
use aoide_melete::{LyricLine, LyricWord, Lyrics, MeleteClient};
use iced::Element;
use iced::widget::column;
use serde::{Deserialize, Serialize};

use crate::app::Message;
use crate::listening::{Msg, SettingsExtras};
use crate::playback::tracks::TrackMeta;

/// How long a lookup is kept.
pub const KEEP_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Lyrics as found, and where from: `None` for the server's own.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub lyrics: Lyrics,
    pub provider: Option<String>,
}

/// One track's lookup, as kept on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cached {
    /// When it was asked, in milliseconds since the epoch.
    pub fetched_ms: i64,
    pub provider: String,
    /// `None`: the provider had nothing for this track.
    pub lines: Option<Vec<CachedLine>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedLine {
    pub text: String,
    #[serde(default)]
    pub start_ms: Option<u64>,
    #[serde(default)]
    pub words: Vec<CachedWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Cached {
    pub fn new(provider: &str, lyrics: Option<&Lyrics>, now_ms: i64) -> Self {
        Self {
            fetched_ms: now_ms,
            provider: provider.to_owned(),
            lines: lyrics.map(|l| {
                l.lines
                    .iter()
                    .map(|line| CachedLine {
                        text: line.text.clone(),
                        start_ms: line.start.map(ms),
                        words: line
                            .words
                            .iter()
                            .map(|w| CachedWord {
                                text: w.text.clone(),
                                start_ms: ms(w.start),
                                end_ms: ms(w.end),
                            })
                            .collect(),
                    })
                    .collect()
            }),
        }
    }

    pub fn lyrics(&self) -> Option<Lyrics> {
        let lines = self.lines.as_ref()?;
        Some(Lyrics {
            lines: lines
                .iter()
                .map(|line| LyricLine {
                    text: line.text.clone(),
                    start: line.start_ms.map(Duration::from_millis),
                    words: line
                        .words
                        .iter()
                        .map(|w| LyricWord {
                            text: w.text.clone(),
                            start: Duration::from_millis(w.start_ms),
                            end: Duration::from_millis(w.end_ms),
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    pub fn is_fresh(&self, now_ms: i64) -> bool {
        now_ms.saturating_sub(self.fetched_ms) < KEEP_MS && self.fetched_ms <= now_ms
    }
}

/// The track's file: its id, kept to letters, digits and dashes.
pub fn cache_file(dir: &Path, item: &str) -> Option<PathBuf> {
    let name: String = item
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    (!name.is_empty()).then(|| dir.join(format!("{name}.json")))
}

/// A fresh lookup for `item`, if one is kept.
pub fn read_cache(dir: &Path, item: &str, now_ms: i64) -> Option<Cached> {
    let bytes = std::fs::read(cache_file(dir, item)?).ok()?;
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    cached.is_fresh(now_ms).then_some(cached)
}

/// Keeps a lookup, through a temporary file. Failing to keep it costs only
/// a lookup next time.
pub fn write_cache(dir: &Path, item: &str, cached: &Cached) {
    let Some(path) = cache_file(dir, item) else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(cached) else {
        return;
    };
    let temporary = path.with_extension("json.tmp");
    let _ = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&temporary, bytes))
        .and_then(|()| std::fs::rename(&temporary, &path));
}

/// Where the lookups are kept: beside the artwork in the cache directory.
pub fn cache_dir(paths: &crate::config::Paths) -> Option<PathBuf> {
    paths.artwork_dir.parent().map(|cache| cache.join("lyrics"))
}

/// What a track is searched by.
pub fn query(meta: &TrackMeta) -> TrackQuery {
    TrackQuery {
        title: meta.title.clone(),
        // The first credited artist: "A, B" was joined for display.
        artist: meta
            .artist
            .split(", ")
            .next()
            .unwrap_or_default()
            .to_owned(),
        album: (!meta.album.is_empty()).then(|| meta.album.clone()),
        duration: meta
            .duration_ms
            .and_then(|ms| u64::try_from(ms).ok())
            .map(Duration::from_millis),
    }
}

/// The internet half of a lookup, given an `Arc` finder (`None`: internet
/// lyrics are off).
pub struct Online {
    pub finder: Arc<LyricsFinder>,
    pub dir: Option<PathBuf>,
}

/// A track's lyrics: the server's, else (when on) the internet's through
/// the cache.
pub async fn find(
    client: MeleteClient,
    online: Option<Online>,
    meta: TrackMeta,
    now_ms: i64,
) -> Result<Option<Found>, aoide_melete::Error> {
    if let Some(lyrics) = client.lyrics(&meta.id).await? {
        return Ok(Some(Found {
            lyrics,
            provider: None,
        }));
    }
    let Some(online) = online else {
        return Ok(None);
    };
    if let Some(cached) = online
        .dir
        .as_deref()
        .and_then(|dir| read_cache(dir, &meta.id, now_ms))
    {
        return Ok(cached.lyrics().map(|lyrics| Found {
            lyrics,
            provider: Some(cached.provider),
        }));
    }
    let provider = online
        .finder
        .providers()
        .first()
        .copied()
        .unwrap_or("LRCLIB");
    match online.finder.find(&query(&meta)).await {
        Ok(found) => {
            let found = found.filter(|f| !f.instrumental && !f.lyrics.is_empty());
            if let Some(dir) = &online.dir {
                let provider = found.as_ref().map_or(provider, |f| f.provider);
                write_cache(
                    dir,
                    &meta.id,
                    &Cached::new(provider, found.as_ref().map(|f| &f.lyrics), now_ms),
                );
            }
            Ok(found.map(|f| Found {
                lyrics: f.lyrics,
                provider: Some(f.provider.to_owned()),
            }))
        }
        // Nobody answered: nothing to keep, nothing to show.
        Err(_) => Ok(None),
    }
}

/// The finder the app keeps: LRCLIB, or in a development build a stand-in
/// named by `AOIDE_DEV_LRCLIB_URL`.
pub fn finder() -> Option<Arc<LyricsFinder>> {
    if cfg!(debug_assertions)
        && let Some(url) = std::env::var_os("AOIDE_DEV_LRCLIB_URL")
            .and_then(|u| u.into_string().ok())
            .and_then(|u| aoide_melete::Url::parse(&u).ok())
    {
        let lrclib = aoide_lyrics::Lrclib::with_base_url(url).ok()?;
        return Some(Arc::new(LyricsFinder::new(vec![Arc::new(lrclib)])));
    }
    LyricsFinder::with_default_providers().ok().map(Arc::new)
}

/// The switch in Settings → Lyrics.
pub fn settings_row<'a>(extras: &SettingsExtras<'a>) -> Element<'a, Message> {
    let interface = extras.config.interface.clone();
    column![
        crate::widgets::switch(
            "Find lyrics on the internet",
            interface.internet_lyrics,
            move |on| {
                let mut change = interface.clone();
                change.internet_lyrics = on;
                Message::Listening(Msg::Interface(change))
            }
        ),
        crate::widgets::secondary(
            "When your server has no lyrics for a song, Aoide asks LRCLIB (lrclib.net) with the \
             song's title, artist, album and length, and keeps what it says on this computer for \
             30 days."
        )
        .size(13),
    ]
    .spacing(6)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn meta(id: &str) -> TrackMeta {
        TrackMeta {
            id: id.into(),
            title: "Harbour Lights".into(),
            artist: "Mara Vey, The Quiet Engines".into(),
            album_artist: None,
            album: "Low Tide".into(),
            album_id: Some("a1".into()),
            artist_id: None,
            artwork: None,
            duration_ms: Some(200_000),
            track_number: None,
            disc_number: None,
            year: None,
            genres: Vec::new(),
            favourite: false,
        }
    }

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    fn online(server: &MockServer, dir: &Path) -> Option<Online> {
        let lrclib =
            aoide_lyrics::Lrclib::with_base_url(aoide_melete::Url::parse(&server.uri()).unwrap())
                .unwrap();
        Some(Online {
            finder: Arc::new(LyricsFinder::new(vec![Arc::new(lrclib)])),
            dir: Some(dir.to_owned()),
        })
    }

    fn synced() -> Lyrics {
        aoide_lyrics::parse_lrc("[00:01.00] one\n[00:04.50] two")
    }

    #[test]
    fn a_lookup_round_trips_through_its_file_and_keeps_thirty_days() {
        let dir = tempfile::tempdir().unwrap();
        let lyrics = synced();
        write_cache(
            dir.path(),
            "abc123",
            &Cached::new("LRCLIB", Some(&lyrics), 1_000),
        );
        let back = read_cache(dir.path(), "abc123", 1_000 + KEEP_MS - 1).unwrap();
        assert_eq!(back.lyrics(), Some(lyrics));
        assert_eq!(back.provider, "LRCLIB");
        assert!(read_cache(dir.path(), "abc123", 1_000 + KEEP_MS).is_none());
        // A clock set back doesn't make a lookup last for ever.
        assert!(read_cache(dir.path(), "abc123", 0).is_none());

        write_cache(dir.path(), "none", &Cached::new("LRCLIB", None, 5));
        let nothing = read_cache(dir.path(), "none", 6).unwrap();
        assert_eq!(nothing.lyrics(), None);
    }

    #[test]
    fn word_timings_survive_the_file() {
        let lyrics = Lyrics {
            lines: vec![LyricLine {
                text: "hello there".into(),
                start: Some(Duration::from_millis(1_000)),
                words: vec![LyricWord {
                    text: "hello".into(),
                    start: Duration::from_millis(1_000),
                    end: Duration::from_millis(1_400),
                }],
            }],
        };
        assert_eq!(
            Cached::new("LRCLIB", Some(&lyrics), 0).lyrics(),
            Some(lyrics)
        );
    }

    #[test]
    fn file_names_are_only_the_ids_letters() {
        let dir = Path::new("/cache/lyrics");
        assert_eq!(cache_file(dir, "ab12-cd"), Some(dir.join("ab12-cd.json")));
        assert_eq!(
            cache_file(dir, "../../etc/passwd"),
            Some(dir.join("etcpasswd.json"))
        );
        assert_eq!(cache_file(dir, "../"), None);
    }

    #[test]
    fn the_query_takes_the_first_artist() {
        let q = query(&meta("t1"));
        assert_eq!(q.artist, "Mara Vey");
        assert_eq!(q.album.as_deref(), Some("Low Tide"));
        assert_eq!(q.duration, Some(Duration::from_secs(200)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_servers_lyrics_come_first_and_lrclib_is_not_asked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks/t1/lyrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"track_id": 1, "source": "file", "synced": true,
                    "lines": [{"start_seconds": 0.0, "text": "from the server"}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let found = find(client(&server), online(&server, dir.path()), meta("t1"), 10)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.provider, None);
        assert_eq!(found.lyrics.lines[0].text, "from the server");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lrclib_fills_in_and_is_asked_once_a_month() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks/t1/lyrics"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .and(query_param("artist_name", "Mara Vey"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "trackName": "Harbour Lights", "artistName": "Mara Vey", "albumName": "Low Tide",
                "duration": 200.0, "instrumental": false, "plainLyrics": "one\ntwo",
                "syncedLyrics": "[00:01.00] one\n[00:04.50] two"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let first = find(client(&server), online(&server, dir.path()), meta("t1"), 10)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.provider.as_deref(), Some("LRCLIB"));
        assert!(first.lyrics.is_synced());
        // Again the next day: from the file, LRCLIB not asked (expect(1)).
        let again = find(
            client(&server),
            online(&server, dir.path()),
            meta("t1"),
            10 + 86_400_000,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(again, first);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nothing_found_is_kept_but_no_answer_is_not() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Audio/t2/Lyrics"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Audio/t3/Lyrics"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .and(query_param("track_name", "Nothing Here"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .and(query_param("track_name", "Nothing Here"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .and(query_param("track_name", "Down"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mut nothing = meta("t2");
        nothing.title = "Nothing Here".into();
        assert_eq!(
            find(client(&server), online(&server, dir.path()), nothing, 10)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            read_cache(dir.path(), "t2", 11).map(|c| c.lines),
            Some(None)
        );

        let mut down = meta("t3");
        down.title = "Down".into();
        assert_eq!(
            find(client(&server), online(&server, dir.path()), down, 10)
                .await
                .unwrap(),
            None
        );
        assert!(read_cache(dir.path(), "t3", 11).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn switched_off_only_the_server_is_asked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks/t1/lyrics"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        assert_eq!(
            find(client(&server), None, meta("t1"), 10).await.unwrap(),
            None
        );
    }
}
