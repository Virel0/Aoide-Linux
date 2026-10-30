//! Playlist import: the shared match table, the iOS `ImportTests.swift` suites, the
//! sidecar's `POST /aoide/match` with the local fallback, and fetching the Spotify
//! embed page from a stand-in server.

use std::sync::Mutex;

use serde_json::{Value, json};
use wiremock::matchers::{body_json, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{
    ImportError, ImportedTrack, LibrarySearch, LibraryTrack, MatchedBy, SidecarClient,
    TrackMatcher, fetch_spotify_playlist_from, match_playlist, parse_playlist_csv,
    parse_spotify_embed, spotify_playlist_id,
};

/// The canonical copy is the desktop repo's `docs/match-table.json`; keep this one
/// byte for byte the same.
const MATCH_TABLE: &str = include_str!("match-table.json");

fn library_track(row: &Value) -> LibraryTrack {
    LibraryTrack {
        jellyfin_id: row["jellyfinId"].as_str().unwrap().to_owned(),
        title: row["title"].as_str().unwrap().to_owned(),
        artists: strings(&row["artists"]),
        album: None,
        duration_ms: row["durationMs"].as_i64(),
        isrc: None,
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

fn candidate(title: &str, artists: &[&str], duration_ms: Option<i64>) -> LibraryTrack {
    LibraryTrack {
        jellyfin_id: format!("lib-{title}"),
        title: title.to_owned(),
        artists: artists.iter().map(|a| (*a).to_owned()).collect(),
        album: None,
        duration_ms,
        isrc: None,
    }
}

// MARK: - The shared table

#[test]
fn every_row_of_the_shared_match_table() {
    let table: Vec<Value> = serde_json::from_str(MATCH_TABLE).unwrap();
    assert_eq!(table.len(), 16);
    for row in &table {
        let imported = &row["import"];
        let track = ImportedTrack {
            title: imported["title"].as_str().unwrap().to_owned(),
            artists: strings(&imported["artists"]),
            album: None,
            duration_ms: imported["durationMs"].as_i64(),
            isrc: None,
        };
        let library: Vec<LibraryTrack> = row["library"]
            .as_array()
            .unwrap()
            .iter()
            .map(library_track)
            .collect();
        let chosen = TrackMatcher::best(&track, &library).map(|i| library[i].jellyfin_id.as_str());
        assert_eq!(chosen, row["expected"].as_str(), "{}", row["name"]);
    }
}

#[test]
fn normalisation_as_specified() {
    for (input, expected) in [
        ("Crazy in Love (feat. Jay-Z)", "crazy in love"),
        ("Karma Police - Remastered 2009", "karma police"),
        (
            "(What's the Story) Morning Glory?",
            "what s the story morning glory",
        ),
        ("Beyoncé & JAY-Z", "beyonce and jay z"),
        ("Song [Live at Wembley]", "song"),
        ("Don't Stop Me Now - 2011 Mix", "don t stop me now"),
        (
            "Ain't No Mountain High Enough",
            "ain t no mountain high enough",
        ),
        ("  Spaced   Out  ", "spaced out"),
    ] {
        assert_eq!(TrackMatcher::normalize(input), expected, "{input}");
    }
}

#[test]
fn search_terms_start_with_the_title_as_spelled_and_widen() {
    let cases: [(&str, &[&str]); 4] = [
        (
            "Don't Stop Me Now - 2011 Mix",
            &[
                "Don't Stop Me Now - 2011 Mix",
                "Don't Stop Me Now",
                "don t stop me now",
                "stop",
            ],
        ),
        ("Karma Police", &["Karma Police", "karma police", "police"]),
        (
            "Crazy in Love (feat. Jay-Z)",
            &[
                "Crazy in Love (feat. Jay-Z)",
                "Crazy in Love",
                "crazy in love",
                "crazy",
            ],
        ),
        ("Hey", &["Hey", "hey"]),
    ];
    for (title, expected) in cases {
        assert_eq!(TrackMatcher::search_terms(title), expected, "{title}");
    }
}

#[test]
fn the_thresholds_are_the_agreed_ones() {
    assert_eq!(TrackMatcher::TITLE_THRESHOLD, 0.75);
    assert_eq!(TrackMatcher::ARTIST_THRESHOLD, 0.7);
    assert_eq!(TrackMatcher::CLOSE_DURATION_MS, 5_000);
    assert_eq!(TrackMatcher::LOOSE_DURATION_MS, 15_000);
    let track = ImportedTrack::new("Karma Police", &["Radiohead"], Some(264_000));
    let close = TrackMatcher::verdict(
        &track,
        &candidate("Karma Police", &["Radiohead"], Some(269_000)),
    );
    assert_eq!(close.duration, 1.0);
    let loose = TrackMatcher::verdict(
        &track,
        &candidate("Karma Police", &["Radiohead"], Some(279_000)),
    );
    assert_eq!(loose.duration, 0.8);
    assert!((loose.overall - (0.5 + 0.35 + 0.15 * 0.8)).abs() < 1e-12);
    let unknown = TrackMatcher::verdict(&track, &candidate("Karma Police", &["Radiohead"], None));
    assert_eq!(unknown.duration, 0.9);
}

/// Rare in Jellyfin tags, certain when present.
#[test]
fn an_isrc_on_both_sides_is_decisive_on_its_own() {
    let mut track = ImportedTrack::new("Totally Different Title", &["Nobody"], Some(1_000));
    track.isrc = Some("USUG12305108".into());
    let mut library = candidate("bad idea right?", &["Olivia Rodrigo"], Some(184_000));
    library.isrc = Some("usug12305108".into());
    let fuzzy = candidate("Totally Different Title", &["Nobody"], Some(1_000));
    let verdict = TrackMatcher::verdict(&track, &library);
    assert!(verdict.is_accepted);
    assert_eq!(TrackMatcher::best(&track, &[fuzzy, library]), Some(1));

    let mut other = candidate("bad idea right?", &["Olivia Rodrigo"], Some(184_000));
    other.isrc = Some("GBXXX0000000".into());
    assert!(!TrackMatcher::verdict(&track, &other).is_accepted);
}

// MARK: - Spotify links and the embed page

#[test]
fn the_id_comes_out_of_every_link_shape() {
    for text in [
        "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc",
        "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
        "https://open.spotify.com/embed/playlist/37i9dQZF1DXcBWIGoYBM5M",
        "  https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M  ",
    ] {
        assert_eq!(
            spotify_playlist_id(text).as_deref(),
            Some("37i9dQZF1DXcBWIGoYBM5M"),
            "{text}"
        );
    }
    for text in [
        "https://open.spotify.com/track/37i9dQZF1DXcBWIGoYBM5M",
        "https://open.spotify.com/playlist/short",
        "hello",
    ] {
        assert_eq!(spotify_playlist_id(text), None, "{text}");
    }
}

const PAGE: &str = r#"<html><head><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"type":"playlist","name":"ignored","title":"Road Trip","subtitle":"Someone","trackList":[{"uri":"spotify:track:1","title":"Karma Police","subtitle":"Radiohead","duration":264066,"isExplicit":false},{"uri":"spotify:track:2","title":"Crazy in Love (feat. Jay-Z)","subtitle":"Beyoncé, JAY-Z","duration":236000},{"uri":"spotify:track:3","title":"","subtitle":"Nobody","duration":1}]}}}}}}</script></head><body></body></html>"#;

#[test]
fn the_embedded_state_is_parsed() {
    let playlist = parse_spotify_embed(PAGE).unwrap();
    assert_eq!(playlist.name, "Road Trip");
    assert_eq!(playlist.tracks.len(), 2, "a row with no title is skipped");
    assert_eq!(
        playlist.tracks[0],
        ImportedTrack::new("Karma Police", &["Radiohead"], Some(264_066))
    );
    assert_eq!(playlist.tracks[1].artists, ["Beyoncé", "JAY-Z"]);
    assert!(!playlist.is_truncated);
}

#[test]
fn a_hundred_tracks_means_there_may_be_more() {
    let rows: Vec<String> = (0..100)
        .map(|i| format!(r#"{{"title":"T{i}","subtitle":"A","duration":1000}}"#))
        .collect();
    let html = format!(
        r#"<script id="__NEXT_DATA__" type="application/json">{{"props":{{"pageProps":{{"state":{{"data":{{"entity":{{"title":"Big","trackList":[{}]}}}}}}}}}}}}</script>"#,
        rows.join(",")
    );
    let playlist = parse_spotify_embed(&html).unwrap();
    assert_eq!(playlist.tracks.len(), 100);
    assert!(playlist.is_truncated);
}

#[test]
fn a_changed_page_says_so_rather_than_returning_nothing() {
    assert_eq!(
        parse_spotify_embed("<html><body>nothing</body></html>"),
        Err(ImportError::SpotifyChanged("no embedded state".into()))
    );
    assert_eq!(
        parse_spotify_embed(
            r#"<script id="__NEXT_DATA__" type="application/json">{"props":{}}</script>"#
        ),
        Err(ImportError::SpotifyChanged(
            "no entity at props.pageProps.state.data.entity".into()
        ))
    );
    assert_eq!(
        parse_spotify_embed(r#"<script id="__NEXT_DATA__" type="application/json">{nope</script>"#),
        Err(ImportError::SpotifyChanged("state is not JSON".into()))
    );
}

/// Only the public page is asked for: a browser's agent, no credentials.
#[tokio::test]
async fn the_embed_page_is_fetched_without_an_account() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/embed/playlist/37i9dQZF1DXcBWIGoYBM5M"))
        .and(header_exists("user-agent"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE))
        .mount(&server)
        .await;
    let playlist = fetch_spotify_playlist_from(
        &server.uri(),
        "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=x",
    )
    .await
    .unwrap();
    assert_eq!(playlist.name, "Road Trip");
    let request = &server.received_requests().await.unwrap()[0];
    assert!(request.headers.get("authorization").is_none());
    assert!(request.headers.get("cookie").is_none());
    assert!(
        request.headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("Mozilla/5.0")
    );

    assert_eq!(
        fetch_spotify_playlist_from(&server.uri(), "spotify:playlist:AAAAAAAAAAAAAAAAAAAAAA").await,
        Err(ImportError::NotShared { status: 404 })
    );
    assert_eq!(
        fetch_spotify_playlist_from(&server.uri(), "not a link").await,
        Err(ImportError::NotASpotifyLink)
    );
}

// MARK: - CSV

#[test]
fn an_exportify_file_reads_quotes_and_all() {
    let csv = "Track URI,Track Name,Artist URI(s),Artist Name(s),Album URI,Album Name,Track Duration (ms),ISRC\n\
        spotify:track:1,\"Crazy in Love (feat. Jay-Z)\",x,\"Beyoncé, JAY-Z\",y,Dangerously in Love,236000,USSM10301234\n\
        spotify:track:2,\"Say \"\"Hi\"\"\",x,Someone,y,\"Album, with comma\",1000,\n";
    let playlist = parse_playlist_csv(csv, "From file").unwrap();
    assert_eq!(playlist.name, "From file");
    assert_eq!(playlist.tracks.len(), 2);
    assert_eq!(
        playlist.tracks[0],
        ImportedTrack {
            title: "Crazy in Love (feat. Jay-Z)".into(),
            artists: vec!["Beyoncé".into(), "JAY-Z".into()],
            album: Some("Dangerously in Love".into()),
            duration_ms: Some(236_000),
            isrc: Some("USSM10301234".into()),
        }
    );
    assert_eq!(playlist.tracks[1].title, "Say \"Hi\"");
    assert_eq!(
        playlist.tracks[1].album.as_deref(),
        Some("Album, with comma")
    );
    assert_eq!(playlist.tracks[1].isrc, None);
    assert!(!playlist.is_truncated);
}

#[test]
fn other_tools_headers_and_minute_second_lengths_are_read() {
    let csv = "Title,Artist,Album,Duration\r\nKarma Police,Radiohead,OK Computer,4:24\r\n";
    let playlist = parse_playlist_csv(csv, "x").unwrap();
    let mut expected = ImportedTrack::new("Karma Police", &["Radiohead"], Some(264_000));
    expected.album = Some("OK Computer".into());
    assert_eq!(playlist.tracks, [expected]);
}

#[test]
fn a_byte_order_mark_and_blank_lines_hide_nothing() {
    let playlist =
        parse_playlist_csv("\u{FEFF}Title,Artist\n\nKarma Police,Radiohead\n,\n", "x").unwrap();
    let titles: Vec<&str> = playlist.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Karma Police"]);
}

#[test]
fn a_file_with_no_title_column_or_nothing_in_it_is_refused() {
    assert_eq!(
        parse_playlist_csv("a,b\n1,2\n", "x"),
        Err(ImportError::NoTitleColumn)
    );
    assert_eq!(parse_playlist_csv("", "x"), Err(ImportError::EmptyCsv));
}

// MARK: - Matching: the sidecar first, the library after

struct Library {
    tracks: Vec<LibraryTrack>,
    searches: Mutex<Vec<String>>,
}

impl LibrarySearch for Library {
    async fn search_songs(&self, term: &str, limit: usize) -> Result<Vec<LibraryTrack>, String> {
        assert_eq!(limit, crate::SEARCH_LIMIT);
        self.searches.lock().unwrap().push(term.to_owned());
        // As the server does: names as typed, case-insensitively.
        let term = term.to_lowercase();
        Ok(self
            .tracks
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&term))
            .cloned()
            .collect())
    }
}

fn library() -> Library {
    Library {
        tracks: vec![
            candidate("Karma Police", &["Radiohead"], Some(264_000)),
            candidate("Don't Stop Me Now", &["Queen"], Some(209_000)),
        ],
        searches: Mutex::new(Vec::new()),
    }
}

fn imported() -> Vec<ImportedTrack> {
    vec![
        ImportedTrack::new(
            "Karma Police - Remastered 2009",
            &["Radiohead"],
            Some(264_000),
        ),
        ImportedTrack::new("Missing Song", &["Nobody"], Some(100_000)),
        ImportedTrack::new("Don't Stop Me Now - 2011 Mix", &["Queen"], Some(210_000)),
    ]
}

#[tokio::test]
async fn the_sidecar_answers_in_one_request_when_it_can() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aoide/match"))
        .and(body_json(json!([
            { "title": "Karma Police - Remastered 2009", "artists": ["Radiohead"], "durationMs": 264000 },
            { "title": "Missing Song", "artists": ["Nobody"], "durationMs": 100000 },
            { "title": "Don't Stop Me Now - 2011 Mix", "artists": ["Queen"], "durationMs": 210000 }
        ])))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "matched": 2, "librarySize": 4213,
            "results": [
                { "jellyfinId": "j1", "confidence": 0.97, "titleScore": 1, "artistScore": 1,
                  "durationScore": 1, "title": "Karma Police", "artists": ["Radiohead"],
                  "album": "OK Computer", "durationMs": 264000 },
                null,
                { "jellyfinId": "j2", "confidence": 0.9 }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let library = library();
    let mut calls = Vec::new();
    let result = match_playlist(Some(&client), &library, &imported(), |done, total| {
        calls.push((done, total));
    })
    .await;
    assert_eq!(result.by, MatchedBy::Sidecar);
    assert_eq!(result.found_ids(), ["j1", "j2"]);
    assert_eq!(result.missing()[0].title, "Missing Song");
    let first = result.matches[0].found.as_ref().unwrap();
    assert_eq!(first.album.as_deref(), Some("OK Computer"));
    assert_eq!(first.score, 0.97);
    assert!(
        library.searches.lock().unwrap().is_empty(),
        "no library searches"
    );
    assert_eq!(calls, [(3, 3)]);
}

#[tokio::test]
async fn without_the_sidecar_the_same_rules_run_over_library_search() {
    let server = MockServer::start().await; // no /aoide/match: an older sidecar
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    for sidecar in [Some(&client), None] {
        let library = library();
        let mut calls = Vec::new();
        let result = match_playlist(sidecar, &library, &imported(), |done, total| {
            calls.push((done, total));
        })
        .await;
        assert_eq!(result.by, MatchedBy::Library);
        assert_eq!(
            result.found_ids(),
            ["lib-Karma Police", "lib-Don't Stop Me Now"]
        );
        assert_eq!(result.missing().len(), 1);
        assert_eq!(calls, [(1, 3), (2, 3), (3, 3)]);
        let searches = library.searches.lock().unwrap().clone();
        // Asked in the title's own spelling first; widened only on a miss.
        assert_eq!(searches[0], "Karma Police - Remastered 2009");
        assert_eq!(searches[1], "Karma Police");
        assert!(searches.contains(&"Don't Stop Me Now".to_owned()));
        assert!(
            !searches.contains(&"don t stop me now".to_owned()),
            "stopped at the first term that found it"
        );
    }
}

#[tokio::test]
async fn a_failing_search_is_a_missing_track_not_a_failed_import() {
    struct Down;
    impl LibrarySearch for Down {
        async fn search_songs(&self, _: &str, _: usize) -> Result<Vec<LibraryTrack>, String> {
            Err("offline".into())
        }
    }
    let result = match_playlist(None, &Down, &imported(), |_, _| {}).await;
    assert_eq!(result.missing().len(), 3);
}

#[test]
fn search_terms_are_cut_to_sixty_characters() {
    assert_eq!(crate::SEARCH_TERM_CHARS, 60);
}
