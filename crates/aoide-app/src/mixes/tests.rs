use super::*;
use aoide_integrations::openrouter::smart_rules::{
    RuleField, RuleOperator, RuleSort, RuleValue, SmartRule, SortDirection,
};

use super::rules::library_matches;

fn track(id: &str, genre: &str, year: i32, favourite: bool) -> TrackMeta {
    TrackMeta {
        id: id.into(),
        title: format!("Song {id}"),
        artist: "Mara Vey, The Quiet Engines".into(),
        album_artist: Some("Mara Vey".into()),
        album: "Harbour Songs".into(),
        album_id: None,
        artist_id: None,
        artwork: None,
        duration_ms: Some(200_000),
        track_number: None,
        disc_number: None,
        year: Some(year),
        genres: vec![genre.into()],
        favourite,
    }
}

fn rule(field: RuleField, op: RuleOperator, value: RuleValue) -> SmartRule {
    SmartRule::new(field, op, value).unwrap()
}

fn library() -> Vec<String> {
    ["Folk", "Hard Rock", "Rock", "Rockabilly", "Ambient"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

fn text(t: &str) -> RuleValue {
    RuleValue::Text(t.into())
}

#[test]
fn genre_words_mean_the_librarys_genres() {
    let rock = rule(RuleField::Genre, RuleOperator::Is, text("rock"));
    let g = library();
    assert_eq!(
        library_matches(&rock, &track("1", "Hard Rock", 2000, false), &g),
        Some(true)
    );
    assert_eq!(
        library_matches(&rock, &track("2", "Rock", 2000, false), &g),
        Some(true)
    );
    assert_eq!(
        library_matches(&rock, &track("3", "Rockabilly", 2000, false), &g),
        Some(false)
    );
    let not_rock = rule(RuleField::Genre, RuleOperator::IsNot, text("rock"));
    assert_eq!(
        library_matches(&not_rock, &track("4", "Folk", 2000, false), &g),
        Some(true)
    );
    assert_eq!(
        library_matches(&not_rock, &track("5", "Hard Rock", 2000, false), &g),
        Some(false)
    );
}

#[test]
fn library_fields_are_checked_on_the_track() {
    let t = track("1", "Folk", 2019, true);
    let g = library();
    let check = |r: SmartRule| library_matches(&r, &t, &g);
    assert_eq!(
        check(rule(
            RuleField::Year,
            RuleOperator::GreaterThan,
            RuleValue::Number(2015)
        )),
        Some(true)
    );
    assert_eq!(
        check(rule(
            RuleField::Year,
            RuleOperator::LessThan,
            RuleValue::Number(2015)
        )),
        Some(false)
    );
    assert_eq!(
        check(rule(
            RuleField::Year,
            RuleOperator::IsNot,
            RuleValue::Number(1999)
        )),
        Some(true)
    );
    // Liked is the store's own like, as the sync log has it.
    assert_eq!(
        check(rule(
            RuleField::Liked,
            RuleOperator::Is,
            RuleValue::Flag(true)
        )),
        None
    );
    assert_eq!(
        check(rule(
            RuleField::Artist,
            RuleOperator::Is,
            text("the quiet engines")
        )),
        Some(true)
    );
    assert_eq!(
        check(rule(
            RuleField::Artist,
            RuleOperator::IsNot,
            text("Mara Vey")
        )),
        Some(false)
    );
    assert_eq!(
        check(rule(
            RuleField::Album,
            RuleOperator::Contains,
            text("harbour")
        )),
        Some(true)
    );
    assert_eq!(
        check(rule(
            RuleField::Title,
            RuleOperator::BeginsWith,
            text("song")
        )),
        Some(true)
    );
    assert_eq!(
        check(rule(
            RuleField::DurationMs,
            RuleOperator::LessThan,
            RuleValue::Number(180_000)
        )),
        Some(false)
    );
    // The listening fields are the store's.
    assert_eq!(
        check(rule(
            RuleField::PlayCount,
            RuleOperator::Is,
            RuleValue::Number(0)
        )),
        None
    );
    assert_eq!(
        check(rule(
            RuleField::LastPlayed,
            RuleOperator::NotInTheLast,
            text("-6m")
        )),
        None
    );
}

fn rules(match_: Match, rules: Vec<SmartRule>) -> SmartRules {
    SmartRules {
        limit: None,
        match_,
        rules,
        sort: None,
    }
}

#[test]
fn the_prefilter_narrows_only_safely() {
    let all = rules(
        Match::All,
        vec![
            rule(RuleField::Genre, RuleOperator::Is, text("rock")),
            rule(
                RuleField::Year,
                RuleOperator::GreaterThan,
                RuleValue::Number(2016),
            ),
            rule(RuleField::Liked, RuleOperator::Is, RuleValue::Flag(true)),
        ],
    );
    let genres = vec!["Hard Rock".to_owned(), "Rock".to_owned()];
    let f = candidate_filter(&all, &genres, &[2023, 2019, 2015]);
    assert_eq!(f.genres, ["Hard Rock", "Rock"]);
    assert_eq!(f.years, [2023, 2019]);
    // Liked is the store's to answer, from the sync log's likes.
    assert!(!f.liked_only);
    // "Any" of a genre and a year: a genre filter would drop the year's
    // tracks, so nothing narrows.
    let any = rules(
        Match::Any,
        vec![
            rule(RuleField::Genre, RuleOperator::Is, text("rock")),
            rule(RuleField::Year, RuleOperator::Is, RuleValue::Number(2019)),
        ],
    );
    let f = candidate_filter(&any, &genres, &[2019]);
    assert!(f.genres.is_empty() && f.years.is_empty() && !f.liked_only);
    // "Any" of genres alone: the genres are already "any of".
    let genres_only = rules(
        Match::Any,
        vec![rule(RuleField::Genre, RuleOperator::Is, text("rock"))],
    );
    assert_eq!(candidate_filter(&genres_only, &genres, &[]).genres.len(), 2);
    // A year rule no library year passes keeps no years (not "every year").
    let none = rules(
        Match::All,
        vec![rule(
            RuleField::Year,
            RuleOperator::LessThan,
            RuleValue::Number(1900),
        )],
    );
    assert!(candidate_filter(&none, &[], &[2019]).years.is_empty());
}

#[test]
fn selection_combines_the_library_the_history_and_the_flags() {
    let candidates = vec![
        track("folk-heard", "Folk", 2019, false),
        track("folk-new", "Folk", 2019, false),
        track("rock-heard", "Rock", 2019, false),
        track("folk-hidden", "Folk", 2019, false),
    ];
    let visible: HashSet<String> = ["folk-heard", "folk-new", "rock-heard"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let heard: HashSet<String> = ["folk-heard", "rock-heard"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let folk = rule(RuleField::Genre, RuleOperator::Is, text("folk"));
    let forgotten = rule(
        RuleField::LastPlayed,
        RuleOperator::NotInTheLast,
        text("-6m"),
    );
    let all = rules(Match::All, vec![folk.clone(), forgotten.clone()]);
    assert_eq!(
        select(Some(&all), &candidates, &library(), Some(&heard), &visible),
        ["folk-heard"]
    );
    let any = rules(Match::Any, vec![folk, forgotten]);
    assert_eq!(
        select(Some(&any), &candidates, &library(), Some(&heard), &visible),
        ["folk-heard", "folk-new", "rock-heard"]
    );
    // "All of nothing" is everything not hidden.
    let everything = rules(Match::All, vec![]);
    assert_eq!(
        select(Some(&everything), &candidates, &library(), None, &visible).len(),
        3
    );
}

#[test]
fn the_listening_rules_go_to_the_store_in_its_own_format() {
    let mixed = SmartRules {
        limit: Some(20),
        match_: Match::All,
        rules: vec![
            rule(RuleField::Genre, RuleOperator::Is, text("folk")),
            rule(
                RuleField::PlayCount,
                RuleOperator::GreaterThan,
                RuleValue::Number(3),
            ),
        ],
        sort: Some(RuleSort {
            dir: SortDirection::Desc,
            field: RuleField::PlayCount,
        }),
    };
    let history = history_rules(&mixed).unwrap();
    assert_eq!(
        history.to_json(),
        r#"{"match":"all","rules":[{"field":"play_count","op":"greaterThan","value":3}],"sort":{"dir":"desc","field":"play_count"}}"#
    );
    let full = store_rules(&mixed).unwrap();
    assert_eq!(full.rules.len(), 2);
    assert_eq!(full.limit, Some(20));
    assert!(
        history_rules(&rules(
            Match::All,
            vec![rule(RuleField::Genre, RuleOperator::Is, text("folk"))]
        ))
        .is_none()
    );
}

#[test]
fn a_library_sort_is_applied_here_and_a_listening_one_by_the_store() {
    let tracks: HashMap<String, TrackMeta> = [
        track("a", "Folk", 2015, false),
        track("b", "Folk", 2023, false),
        track("c", "Folk", 2019, false),
    ]
    .into_iter()
    .map(|t| (t.id.clone(), t))
    .collect();
    let ids = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let newest = SmartRules {
        sort: Some(RuleSort {
            dir: SortDirection::Desc,
            field: RuleField::Year,
        }),
        ..rules(Match::All, vec![])
    };
    assert_eq!(
        sorted(ids.clone(), Some(&newest), &tracks, &[]),
        ["b", "c", "a"]
    );
    let most_played = SmartRules {
        sort: Some(RuleSort {
            dir: SortDirection::Desc,
            field: RuleField::PlayCount,
        }),
        ..rules(Match::All, vec![])
    };
    let store_order = vec!["c".to_owned(), "a".to_owned()];
    assert_eq!(
        sorted(ids.clone(), Some(&most_played), &tracks, &store_order),
        ["c", "a", "b"]
    );
    assert_eq!(sorted(ids.clone(), None, &tracks, &[]), ids);
}

#[test]
fn a_mix_is_kept_only_with_rules_that_say_something() {
    let mix = |rules| Mix {
        description: "x".into(),
        looked_for: String::new(),
        tracks: Vec::new(),
        rules,
        rejected: Vec::new(),
        missed: None,
    };
    assert!(!mix(None).can_save());
    assert!(!mix(Some(rules(Match::All, vec![]))).can_save());
    assert!(
        mix(Some(rules(
            Match::All,
            vec![rule(RuleField::Genre, RuleOperator::Is, text("folk"))]
        )))
        .can_save()
    );
}

#[tokio::test]
async fn liked_is_read_from_the_stores_own_likes() {
    let liked = rules(
        Match::All,
        vec![rule(
            RuleField::Liked,
            RuleOperator::Is,
            RuleValue::Flag(true),
        )],
    );
    let candidates = [
        track("liked", "Folk", 2019, false),
        track("favourite", "Folk", 2019, true),
    ];
    let history = history_rules(&liked).expect("liked goes to the store");
    let now = aoide_store::now_millis();
    let rows: Vec<_> = candidates.iter().map(|t| t.track_ref(now)).collect();
    let store = StoreHandle::in_memory("device-1").unwrap();
    let heard: HashSet<String> = store
        .call(move |w| {
            w.store.cache_tracks(&rows).unwrap();
            let like = aoide_store::Like::new("liked", "liked", true, 0, "");
            w.store.record(like, false, now).unwrap();
            w.store
                .evaluate(&history, now, aoide_store::Calendar::Local)
                .unwrap()
        })
        .await
        .unwrap()
        .into_iter()
        .collect();
    let visible: HashSet<String> = candidates.iter().map(|t| t.id.clone()).collect();
    let kept = select(Some(&liked), &candidates, &[], Some(&heard), &visible);
    assert_eq!(kept, ["liked"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mood_becomes_a_mix_with_the_names_first_and_the_hidden_left_out() {
    use crate::credentials::MemoryStore;
    use crate::test_json::{page, track as json_track};
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let get = |p: &str| Mock::given(method("GET")).and(path(p.to_owned()));
    let ok = |body: serde_json::Value| ResponseTemplate::new(200).set_body_json(body);
    let song = |id: i64, genre: &str, album: &str| {
        let mut song = json_track(id, &format!("Song {id}"));
        song["genres"] = json!([genre]);
        song["album"] = json!(album);
        song["album_artist"] = json!("Someone");
        song["year"] = json!(2019);
        song
    };
    let album = |id: i64, title: &str| {
        json!({"id": id, "title": title, "artist_id": 7, "artist": "Mara Vey", "year": 2019,
               "tracks": 2, "duration_seconds": 400.0, "cover": null})
    };
    get("/api/v1/music/genres")
        .respond_with(ok(json!([
            {"name": "Folk", "tracks": 3, "albums": 1}, {"name": "Rock", "tracks": 1, "albums": 1}
        ])))
        .mount(&server)
        .await;
    // The name: an artist with one album of two songs, and no songs or
    // albums by that name.
    get("/api/v1/music/albums")
        .and(query_param("q", "Mara Vey"))
        .respond_with(ok(page(vec![])))
        .mount(&server)
        .await;
    get("/api/v1/music/albums")
        .respond_with(ok(page(vec![album(9, "Saltwater")])))
        .mount(&server)
        .await;
    get("/api/v1/music/artists")
        .and(query_param("q", "Mara Vey"))
        .respond_with(ok(page(vec![
            json!({"id": 7, "name": "Mara Vey", "mb_artist_id": null, "albums": 1, "appears_on": 0}),
        ])))
        .mount(&server)
        .await;
    get("/api/v1/music/artists/7")
        .respond_with(ok(
            json!({"id": 7, "name": "Mara Vey", "mb_artist_id": null, "albums": 1,
            "appears_on": 0, "album_list": [album(9, "Saltwater")], "appears_on_list": []}),
        ))
        .mount(&server)
        .await;
    let mut saltwater = album(9, "Saltwater");
    saltwater["track_list"] = json!([song(11, "Folk", "Saltwater"), song(12, "Folk", "Saltwater")]);
    get("/api/v1/music/albums/9")
        .respond_with(ok(saltwater))
        .mount(&server)
        .await;
    get("/api/v1/music/tracks")
        .and(query_param("q", "Mara Vey"))
        .respond_with(ok(page(vec![])))
        .mount(&server)
        .await;
    // The rules' candidates: folk, one of them marked Not Interested.
    get("/api/v1/music/tracks")
        .and(query_param("genre", "Folk"))
        .respond_with(ok(page(vec![
            song(21, "Folk", "A"),
            song(22, "Folk", "B"),
            song(23, "Folk", "C"),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let reply = json!({"match": "all", "limit": 4, "names": ["Mara Vey"],
        "rules": [{"field": "genre", "op": "is", "value": "folk"},
                  {"field": "last_played", "op": "notInTheLast", "value": "-6m"},
                  {"field": "duration_ms", "op": "beginsWith", "value": 3}]});
    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": reply.to_string()}}]
        })))
        .mount(&server)
        .await;

    let store = StoreHandle::in_memory("device-1").unwrap();
    store
        .call(|w| {
            w.store
                .set_not_interested(true, "22", "key-hidden", aoide_store::now_millis())
                .unwrap();
        })
        .await
        .unwrap();
    let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    keyring
        .set(aoide_integrations::openrouter::KEYRING_ENTRY, "sk-or-test")
        .unwrap();
    let client = MeleteClient::new(
        aoide_melete::ServerAddress::parse(&server.uri()).unwrap(),
        aoide_melete::Device::new("Test", "device-1"),
    )
    .unwrap()
    .with_credentials(aoide_melete::Credentials {
        user_id: "1".into(),
        access_token: "tok".into(),
        username: "tester".into(),
    });
    let mix = compose(
        OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap(),
        keyring,
        "test/model".into(),
        client,
        store.clone(),
        "forgotten folk and Mara Vey".into(),
    )
    .await
    .unwrap();
    let ids: Vec<&str> = mix.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(&ids[..2], ["11", "12"], "the names come first: {ids:?}");
    assert_eq!(ids.len(), 4, "cut at the rules' limit: {ids:?}");
    assert!(!ids.contains(&"22"), "{ids:?}");
    assert_eq!(
        mix.looked_for,
        "Mara Vey · Folk · not played in 6 months · 4 tracks"
    );
    assert_eq!(mix.rejected.len(), 1, "{:?}", mix.rejected);
    assert!(mix.missed.is_none());
    assert!(mix.can_save());
    // The candidates went into the store's track cache for the history.
    let cached = store
        .call(|w| w.store.cached_track_count().unwrap())
        .await
        .unwrap();
    assert_eq!(cached, 5);
}

#[test]
fn playlist_names_come_from_the_description() {
    assert_eq!(
        playlist_name("  rainy Sunday folk \nmore"),
        "rainy Sunday folk"
    );
    assert_eq!(playlist_name("   "), "Mix");
    let long = "a".repeat(80);
    assert_eq!(playlist_name(&long).chars().count(), 61);
}
