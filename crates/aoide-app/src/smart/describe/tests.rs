use super::*;

fn query() -> MusicFilters {
    MusicFilters::default()
}

fn genres() -> Vec<String> {
    ["Folk", "Indie", "Ambient"].map(String::from).to_vec()
}

#[test]
fn filters_read_as_words() {
    let q = MusicFilters {
        genres: vec!["Folk".into(), "Indie".into()],
        from_year: Some(2010),
        to_year: Some(2019),
        liked_only: true,
        ordering: MusicOrdering::NewestFirst,
        search_term: Some("harbour".into()),
    };
    assert_eq!(
        understood(&q),
        "“harbour” · Folk, Indie · 2010–2019 · liked · newest first"
    );
    let one_year = MusicFilters {
        from_year: Some(1999),
        to_year: Some(1999),
        ..query()
    };
    assert_eq!(understood(&one_year), "1999");
    let open = MusicFilters {
        from_year: Some(2020),
        ordering: MusicOrdering::Random,
        ..query()
    };
    assert_eq!(understood(&open), "from 2020 · in random order");
}

#[test]
fn years_come_from_the_library_inside_the_range() {
    let library = [2023, 2022, 2021, 2019, 2018, 2015];
    let q = MusicFilters {
        from_year: Some(2016),
        to_year: Some(2021),
        ..query()
    };
    assert_eq!(
        years_in_range(&q, &library, 2026),
        Some(vec![2018, 2019, 2021])
    );
    assert_eq!(years_in_range(&query(), &library, 2026), None);
    // Nothing in range: an empty list, which the query must not send.
    let none = MusicFilters {
        from_year: Some(1960),
        to_year: Some(1969),
        ..query()
    };
    assert_eq!(years_in_range(&none, &library, 2026), Some(vec![]));
    // A server that lists no years: the range spelled out, up to now.
    let open = MusicFilters {
        from_year: Some(2024),
        ..query()
    };
    assert_eq!(
        years_in_range(&open, &[], 2026),
        Some(vec![2024, 2025, 2026])
    );
    let up_to = MusicFilters {
        to_year: Some(1999),
        ..query()
    };
    let spelled = years_in_range(&up_to, &[], 2026).unwrap();
    assert_eq!(spelled.len(), MAX_YEARS as usize);
    assert_eq!(spelled.last(), Some(&1999));
}

#[test]
fn the_filter_carries_every_filter() {
    let q = MusicFilters {
        genres: vec!["folk".into(), "Jazz".into()],
        from_year: Some(2018),
        to_year: Some(2019),
        liked_only: true,
        ordering: MusicOrdering::MostPlayed,
        search_term: Some("lights".into()),
    };
    let filter = song_filter(&q, &genres(), &[2019, 2015], 2026).unwrap();
    assert_eq!(
        filter.genres,
        ["Folk"],
        "as the library spells it; Jazz it hasn't"
    );
    assert_eq!(filter.years, [2019]);
    assert!(filter.liked_only);
    assert_eq!(filter.term.as_deref(), Some("lights"));
    assert_eq!(filter.sort, Sort::Default);
    let by_name = MusicFilters {
        ordering: MusicOrdering::ByName,
        ..q.clone()
    };
    assert_eq!(
        song_filter(&by_name, &genres(), &[2019], 2026)
            .unwrap()
            .sort,
        Sort::Name
    );
    let out_of_range = MusicFilters {
        from_year: Some(1950),
        to_year: Some(1959),
        ..q
    };
    assert_eq!(song_filter(&out_of_range, &genres(), &[2019], 2026), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn orders_melete_cant_sort_by_are_put_here() {
    let song = |id: &str, year: i32| Item {
        id: id.into(),
        year: Some(year),
        ..Item::default()
    };
    let store = StoreHandle::in_memory("desk").unwrap();
    let songs = vec![song("1", 2001), song("2", 2020), song("3", 2010)];
    let newest = ordered(songs.clone(), MusicOrdering::NewestFirst, &store).await;
    let ids: Vec<&str> = newest.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["2", "3", "1"]);
    let random = ordered(songs, MusicOrdering::Random, &store).await;
    assert_eq!(random.len(), 3);
}

fn melete(server: &wiremock::MockServer) -> MeleteClient {
    MeleteClient::new(
        aoide_melete::ServerAddress::parse(&server.uri()).unwrap(),
        aoide_melete::Device::new("Test", "device-1"),
    )
    .unwrap()
    .with_credentials(aoide_melete::Credentials {
        user_id: "u1".into(),
        access_token: "tok".into(),
        username: "tester".into(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_description_becomes_a_melete_query_and_only_words_leave() {
    use crate::test_json::{page, track};
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/genres"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"name": "Folk", "tracks": 3, "albums": 1}, {"name": "Indie", "tracks": 1, "albums": 1}
        ])))
        .mount(&server)
        .await;
    let album = |id: i64, year: i32| {
        serde_json::json!({"id": id, "title": "A", "artist_id": 1, "artist": "B",
            "year": year, "tracks": 1, "duration_seconds": 1.0, "cover": null})
    };
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![
            album(1, 2015),
            album(2, 2019),
            album(3, 2022),
        ])))
        .mount(&server)
        .await;
    let mut lights = track(1, "Harbour Lights");
    lights["year"] = serde_json::json!(2019);
    let mut late = track(2, "Later");
    late["year"] = serde_json::json!(2022);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("genre", "Folk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![lights, late])))
        .expect(1)
        .mount(&server)
        .await;
    // The server's AI answers; only the phrase is asked with. Its years are
    // kept because the phrase has a number ("the twenty tens" has none, and
    // would lose them: see `grounding`).
    Mock::given(method("POST"))
        .and(path("/api/v1/ai/music/search"))
        .and(body_json(
            serde_json::json!({"query": "calm folk from the 2010s"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "search_term": null, "genres": ["Folk"], "from_year": 2010,
            "to_year": 2019, "liked_only": false, "ordering": "by_name"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let found = search_by_description(
        melete(&server),
        StoreHandle::in_memory("desk").unwrap(),
        "calm folk from the 2010s".into(),
    )
    .await
    .unwrap();
    assert_eq!(found.understood.as_deref(), Some("Folk · 2010–2019"));
    assert_eq!(found.songs.len(), 1);
    assert_eq!(found.songs[0].title, "Harbour Lights");
    assert_eq!(found.note, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guess_the_words_dont_say_leaves_the_plain_search() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ai/music/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "search_term": null, "genres": ["Alternative"], "from_year": null,
            "to_year": null, "liked_only": false, "ordering": "by_name"
        })))
        .mount(&server)
        .await;
    let found = search_by_description(
        melete(&server),
        StoreHandle::in_memory("desk").unwrap(),
        "Karma Police Radiohead".into(),
    )
    .await
    .unwrap();
    assert_eq!(found.understood, None);
    assert!(found.songs.is_empty());
    assert!(found.note.unwrap().contains("plain search stands"));
    // Nothing to filter by: the library wasn't asked.
    let asked = server.received_requests().await.unwrap();
    assert_eq!(asked.len(), 1, "{asked:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_music_ai_or_with_it_off_is_learnt() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    for (status, ai) in [(404, Ai::Missing), (503, Ai::Off)] {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/ai/music/search"))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(serde_json::json!({"error": "no"})),
            )
            .mount(&server)
            .await;
        let failed = search_by_description(
            melete(&server),
            StoreHandle::in_memory("desk").unwrap(),
            "calm folk from the twenty tens".into(),
        )
        .await
        .unwrap_err();
        assert_eq!(failed.availability(), Some(ai), "HTTP {status}");
        assert!(!failed.is_unauthorized());
    }
}

#[test]
fn other_failures_say_nothing_of_the_ai() {
    let busy = Failure::Ai(Error::Http {
        status: 429,
        path: "api/v1/ai/music/search".into(),
        detail: None,
        reason: None,
    });
    assert_eq!(busy.availability(), None);
    let library = Failure::Server(Error::NotFound {
        path: "api/v1/music/genres".into(),
    });
    assert_eq!(library.availability(), None);
    assert!(Failure::Ai(Error::Unauthorized).is_unauthorized());
    assert!(Failure::Server(Error::Unauthorized).is_unauthorized());
}

#[test]
fn this_year_is_plausible() {
    let year = this_year();
    assert!((2025..2200).contains(&year), "{year}");
}
