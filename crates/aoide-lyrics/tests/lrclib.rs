//! LRCLIB and the finder against a local mock server. Nothing here leaves 127.0.0.1.

use std::sync::Arc;
use std::time::Duration;

use aoide_lyrics::{
    BoxFuture, Candidate, Lrclib, LyricsFinder, LyricsProvider, ProviderError, TrackQuery,
    USER_AGENT, parse_lrc, parse_plain,
};
use reqwest::Url;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn track() -> TrackQuery {
    TrackQuery {
        title: "Harbour Lights".into(),
        artist: "The Quiet Engines".into(),
        album: Some("Low Tide Radio".into()),
        duration: Some(Duration::from_millis(243_400)),
    }
}

fn record(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Value {
    json!({
        "id": 1,
        "name": "Harbour Lights",
        "trackName": "Harbour Lights",
        "artistName": "The Quiet Engines",
        "albumName": "Low Tide Radio",
        "duration": duration,
        "instrumental": false,
        "hasWordSync": false,
        "plainLyrics": plain,
        "syncedLyrics": synced,
        "lyricsfile": "version: 1\n"
    })
}

fn finder(server: &MockServer) -> LyricsFinder {
    let lrclib = Lrclib::with_base_url(Url::parse(&server.uri()).unwrap()).unwrap();
    LyricsFinder::new(vec![Arc::new(lrclib)])
}

fn query_value(request: &Request, name: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

#[tokio::test]
async fn an_exact_match_is_one_request_that_names_the_app() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .and(query_param("track_name", "Harbour Lights"))
        .and(query_param("artist_name", "The Quiet Engines"))
        .and(query_param("album_name", "Low Tide Radio"))
        .and(query_param("duration", "243"))
        .and(header("User-Agent", USER_AGENT))
        .respond_with(ResponseTemplate::new(200).set_body_json(record(
            243.0,
            Some("[00:01.00]First\n[00:04.20]Second\n"),
            Some("First\nSecond\n"),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let found = finder(&server).find(&track()).await.unwrap().unwrap();
    assert_eq!(found.provider, "LRCLIB");
    assert!(found.lyrics.is_synced());
    assert_eq!(found.lyrics.lines[1].text, "Second");
    assert!(!found.instrumental);
    assert_eq!(found.duration_gap, Some(Duration::from_millis(400)));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(USER_AGENT.starts_with("Aoide for Linux/"));
    assert!(USER_AGENT.ends_with("(https://github.com/Virel0/aoide-linux)"));
}

#[tokio::test]
async fn no_exact_match_falls_back_to_a_search_and_drops_other_recordings() {
    let server = MockServer::start().await;
    Mock::given(path("/api/get"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "code": 404, "name": "TrackNotFound", "message": "Failed to find specified track"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .and(query_param("track_name", "Harbour Lights"))
        .and(query_param("artist_name", "The Quiet Engines"))
        .and(|request: &Request| query_value(request, "album_name").is_none())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            // A live take, 40 s longer: synced, but the wrong recording.
            record(283.0, Some("[00:01.00]Live\n"), Some("Live\n")),
            // No words at all, and not instrumental: no candidate.
            record(243.0, None, None),
            // The right length, plain text only.
            record(242.0, None, Some("Plain words\n")),
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let found = finder(&server).find(&track()).await.unwrap().unwrap();
    assert!(!found.lyrics.is_synced());
    assert_eq!(found.lyrics.lines[0].text, "Plain words");
    let order: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(order, ["/api/get", "/api/search"]);
}

#[tokio::test]
async fn nothing_anywhere_is_none() {
    let server = MockServer::start().await;
    Mock::given(path("/api/get"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    assert_eq!(finder(&server).find(&track()).await, Ok(None));
}

#[tokio::test]
async fn an_instrumental_is_found_with_no_lines() {
    let server = MockServer::start().await;
    let mut instrumental = record(243.0, None, None);
    instrumental["instrumental"] = json!(true);
    Mock::given(path("/api/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(instrumental))
        .mount(&server)
        .await;
    let found = finder(&server).find(&track()).await.unwrap().unwrap();
    assert!(found.instrumental);
    assert!(found.lyrics.is_empty());
}

#[tokio::test]
async fn a_429_is_honoured_and_nothing_is_sent_until_the_wait_is_over() {
    let server = MockServer::start().await;
    Mock::given(path("/api/get"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "120")
                .set_body_json(json!({ "name": "RateLimitExceeded", "statusCode": 429 })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let finder = finder(&server);
    for _ in 0..2 {
        let failed = finder.find(&track()).await.unwrap_err();
        let [(name, ProviderError::RateLimited { retry_after })] = failed.0.as_slice() else {
            panic!("expected one rate limit, got {failed:?}");
        };
        assert_eq!(*name, "LRCLIB");
        // 120 s, plus up to 30 % jitter, less what has passed.
        assert!(*retry_after > Duration::from_secs(110), "{retry_after:?}");
        assert!(*retry_after <= Duration::from_secs(156), "{retry_after:?}");
    }
    // The second lookup never reached the server.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_slow_provider_times_out_and_a_failing_one_is_an_error() {
    let slow = MockServer::start().await;
    Mock::given(path("/api/get"))
        .respond_with(ResponseTemplate::new(404).set_delay(Duration::from_secs(5)))
        .mount(&slow)
        .await;
    let started = std::time::Instant::now();
    let failed = finder(&slow)
        .timeout(Duration::from_millis(200))
        .find(&track())
        .await
        .unwrap_err();
    assert_eq!(failed.0, [("LRCLIB", ProviderError::TimedOut)]);
    assert!(started.elapsed() < Duration::from_secs(2));

    let broken = MockServer::start().await;
    Mock::given(path("/api/get"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&broken)
        .await;
    assert_eq!(
        finder(&broken).find(&track()).await.unwrap_err().0,
        [("LRCLIB", ProviderError::Http { status: 500 })]
    );
}

#[tokio::test]
async fn nothing_is_sent_until_a_lookup_and_not_for_an_unsearchable_track() {
    let server = MockServer::start().await;
    let finder = finder(&server);
    assert_eq!(finder.providers(), ["LRCLIB"]);
    let nameless = TrackQuery {
        artist: "  ".into(),
        ..track()
    };
    assert_eq!(finder.find(&nameless).await, Ok(None));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_track_query_comes_from_a_jellyfin_item() {
    let item: aoide_jellyfin::Item = serde_json::from_value(json!({
        "Id": "t1", "Name": "Harbour Lights", "Album": "Low Tide Radio",
        "AlbumArtist": "Various Artists", "Artists": ["The Quiet Engines", "Mara Vey"],
        "RunTimeTicks": 2_434_000_000_i64
    }))
    .unwrap();
    assert_eq!(TrackQuery::from_item(&item), track());
}

// MARK: - Ranking, with stand-in providers

struct Fixed {
    name: &'static str,
    answer: Result<Vec<Candidate>, ProviderError>,
}

impl LyricsProvider for Fixed {
    fn name(&self) -> &'static str {
        self.name
    }

    fn search<'a>(
        &'a self,
        _track: &'a TrackQuery,
    ) -> BoxFuture<'a, Result<Vec<Candidate>, ProviderError>> {
        let answer = self.answer.clone();
        Box::pin(async move { answer })
    }
}

fn candidate(lyrics: &str, seconds: Option<u64>) -> Candidate {
    Candidate {
        lyrics: if lyrics.starts_with('[') {
            parse_lrc(lyrics)
        } else {
            parse_plain(lyrics)
        },
        instrumental: false,
        duration: seconds.map(Duration::from_secs),
        title: "Harbour Lights".into(),
        artist: "The Quiet Engines".into(),
        album: None,
    }
}

fn fixed(
    name: &'static str,
    answer: Result<Vec<Candidate>, ProviderError>,
) -> Arc<dyn LyricsProvider> {
    Arc::new(Fixed { name, answer })
}

async fn winner(providers: Vec<Arc<dyn LyricsProvider>>) -> (String, String) {
    let found = LyricsFinder::new(providers)
        .find(&track())
        .await
        .unwrap()
        .unwrap();
    (
        found.provider.to_owned(),
        found.lyrics.lines[0].text.clone(),
    )
}

#[tokio::test]
async fn synced_lyrics_beat_plain_ones_from_a_preferred_provider() {
    let won = winner(vec![
        fixed("first", Ok(vec![candidate("plain", Some(243))])),
        fixed("second", Ok(vec![candidate("[00:01.00]synced", Some(243))])),
    ])
    .await;
    assert_eq!(won, ("second".to_owned(), "synced".to_owned()));
}

#[tokio::test]
async fn word_timing_beats_line_timing() {
    let won = winner(vec![
        fixed("first", Ok(vec![candidate("[00:01.00]lines", Some(243))])),
        fixed(
            "second",
            Ok(vec![candidate(
                "[00:01.00]<00:01.00>words<00:01.50>",
                Some(243),
            )]),
        ),
    ])
    .await;
    assert_eq!(won.1, "words");
}

#[tokio::test]
async fn between_equals_the_preferred_provider_wins_then_the_closer_duration() {
    let won = winner(vec![
        fixed("first", Ok(vec![candidate("[00:01.00]a", Some(241))])),
        fixed("second", Ok(vec![candidate("[00:01.00]b", Some(243))])),
    ])
    .await;
    assert_eq!(won.0, "first");

    let won = winner(vec![fixed(
        "only",
        Ok(vec![
            candidate("[00:01.00]unknown length", None),
            candidate("[00:01.00]two off", Some(241)),
            candidate("[00:01.00]right on", Some(243)),
        ]),
    )])
    .await;
    assert_eq!(won.1, "right on");
}

#[tokio::test]
async fn one_provider_failing_doesnt_sink_the_others() {
    let won = winner(vec![
        fixed("down", Err(ProviderError::Transport("refused".into()))),
        fixed("up", Ok(vec![candidate("[00:01.00]here", Some(243))])),
    ])
    .await;
    assert_eq!(won.0, "up");

    let all_down = LyricsFinder::new(vec![
        fixed("a", Err(ProviderError::TimedOut)),
        fixed("b", Err(ProviderError::Http { status: 503 })),
    ])
    .find(&track())
    .await
    .unwrap_err();
    assert_eq!(all_down.0.len(), 2);
    assert_eq!(
        all_down.to_string(),
        "no lyrics provider answered: a: no answer in time; b: HTTP 503"
    );
}
