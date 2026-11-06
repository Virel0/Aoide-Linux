//! A song whose artist is only a YouTube channel's name, found in the library by its
//! title alone: the iOS `TitleAloneTests.swift`.

use std::sync::Mutex;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::import::title_alone::by_title_alone;
use crate::{ImportedTrack, LibrarySearch, LibraryTrack, SidecarClient, match_playlist};

fn song(id: &str, title: &str, artist: &str) -> LibraryTrack {
    LibraryTrack {
        jellyfin_id: id.to_owned(),
        title: title.to_owned(),
        artists: vec![artist.to_owned()],
        album: None,
        duration_ms: Some(150_000),
        isrc: None,
    }
}

/// "Fish in the Sea" from a label's channel, the artist only a guess.
fn from_a_label(title: &str) -> ImportedTrack {
    ImportedTrack {
        artist_from_channel: true,
        ..ImportedTrack::new(title, &["Ubisoft Music"], None)
    }
}

struct Library(Vec<LibraryTrack>, Mutex<usize>);

impl LibrarySearch for Library {
    async fn search_songs(&self, term: &str, _: usize) -> Result<Vec<LibraryTrack>, String> {
        *self.1.lock().unwrap() += 1;
        let term = term.to_lowercase();
        Ok(self
            .0
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&term))
            .cloned()
            .collect())
    }
}

#[test]
fn only_a_guessed_artist_and_one_artist_with_exactly_the_title_will_do() {
    let one = [
        song("a", "Fish in the Sea", "Assassin's Creed Shanties"),
        song("b", "Fish in the Sea (Live)", "Assassin's Creed Shanties"),
        song("c", "Big Fish in the Sea", "Someone Else"),
    ];
    assert_eq!(
        by_title_alone(&from_a_label("Fish in the Sea"), &one),
        Some(0)
    );
    let named = ImportedTrack::new("Fish in the Sea", &["Ubisoft Music"], None);
    assert_eq!(
        by_title_alone(&named, &one),
        None,
        "an artist the source named stands"
    );
    let two = [
        song("a", "Fish in the Sea", "Assassin's Creed Shanties"),
        song("d", "Fish in the Sea", "Another Band"),
    ];
    assert_eq!(
        by_title_alone(&from_a_label("Fish in the Sea"), &two),
        None,
        "two artists: which one the video is can't be told"
    );
    assert_eq!(by_title_alone(&from_a_label("Fish"), &one), None);
}

#[tokio::test]
async fn a_song_the_library_has_under_another_artist_is_found_and_takes_its_artist() {
    // The server's match finds nothing: "Ubisoft Music" isn't the library's artist.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aoide/match"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [null, null] })))
        .expect(1)
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let library = Library(
        vec![song(
            "lib-1",
            "Fish in the Sea",
            "Assassin's Creed Shanties",
        )],
        Mutex::new(0),
    );
    let tracks = [
        from_a_label("Fish in the Sea"),
        ImportedTrack::new("Fish in the Sea", &["Nobody Here"], None),
    ];
    let result = match_playlist(Some(&client), &library, &tracks, |_, _| {}).await;

    assert_eq!(result.found_ids(), ["lib-1"]);
    let found = &result.matches[0];
    assert_eq!(
        found.track.artists,
        ["Assassin's Creed Shanties"],
        "sent to Melete as the library's"
    );
    assert!(!found.track.artist_from_channel);
    assert!(
        result.matches[1].found.is_none(),
        "a named artist isn't second-guessed"
    );
    assert_eq!(
        *library.1.lock().unwrap(),
        1,
        "only the guessed one is looked up"
    );
}
