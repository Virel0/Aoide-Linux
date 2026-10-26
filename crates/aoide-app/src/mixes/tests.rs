use super::*;
use aoide_integrations::openrouter::smart_rules::{
    RuleField, RuleOperator, RuleSort, RuleValue, SmartRule, SortDirection,
};
use aoide_jellyfin::{NameId, Sort};

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
    // Liked is the store's (Aoide's own like), not the Jellyfin favourite.
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

fn ids() -> Vec<NameId> {
    library()
        .into_iter()
        .map(|name| NameId {
            id: format!("g-{name}"),
            name,
        })
        .collect()
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
    let q = candidate_query(&all, &genres, &ids(), &[2023, 2019, 2015], Some("lib1"));
    assert_eq!(q.genre_ids, ["g-Hard Rock", "g-Rock"]);
    assert_eq!(q.years, [2023, 2019]);
    // Liked is Aoide's like, which Jellyfin's favourites can't narrow.
    assert!(!q.favorites_only);
    assert_eq!(q.sort, Sort::Random);
    assert_eq!(q.parent_id.as_deref(), Some("lib1"));
    // "Any" of a genre and a year: a genre filter would drop the year's
    // tracks, so nothing narrows.
    let any = rules(
        Match::Any,
        vec![
            rule(RuleField::Genre, RuleOperator::Is, text("rock")),
            rule(RuleField::Year, RuleOperator::Is, RuleValue::Number(2019)),
        ],
    );
    let q = candidate_query(&any, &genres, &ids(), &[2019], None);
    assert!(q.genre_ids.is_empty() && q.years.is_empty() && !q.favorites_only);
    // "Any" of genres alone: the genre ids are already "any of".
    let genres_only = rules(
        Match::Any,
        vec![rule(RuleField::Genre, RuleOperator::Is, text("rock"))],
    );
    assert_eq!(
        candidate_query(&genres_only, &genres, &ids(), &[], None)
            .genre_ids
            .len(),
        2
    );
    // A year rule no library year passes sends no years (not "every year").
    let none = rules(
        Match::All,
        vec![rule(
            RuleField::Year,
            RuleOperator::LessThan,
            RuleValue::Number(1900),
        )],
    );
    assert!(
        candidate_query(&none, &[], &ids(), &[2019], None)
            .years
            .is_empty()
    );
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
async fn liked_is_aoides_own_like_not_the_jellyfin_favourite() {
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
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let get = |p: &str| Mock::given(method("GET")).and(path(p.to_owned()));
    let items = |items: serde_json::Value| {
        let n = items.as_array().map_or(0, Vec::len);
        ResponseTemplate::new(200).set_body_json(json!({"Items": items, "TotalRecordCount": n}))
    };
    let song = |id: &str, genre: &str, album: &str| {
        json!({"Id": id, "Name": format!("Song {id}"), "Type": "Audio", "Genres": [genre],
               "Album": album, "AlbumArtist": "Someone", "ProductionYear": 2019})
    };
    get("/Items/Filters2")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Genres": [{"Id": "g-folk", "Name": "Folk"}, {"Id": "g-rock", "Name": "Rock"}]
        })))
        .mount(&server)
        .await;
    get("/Items/Filters")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Years": [2019]})))
        .mount(&server)
        .await;
    // The name: an album artist with one album of two songs.
    get("/Artists/AlbumArtists")
        .and(query_param("searchTerm", "Mara Vey"))
        .respond_with(items(
            json!([{"Id": "ar-mv", "Name": "Mara Vey", "Type": "MusicArtist"}]),
        ))
        .mount(&server)
        .await;
    get("/Items")
        .and(query_param("albumArtistIds", "ar-mv"))
        .respond_with(items(
            json!([{"Id": "al-mv", "Name": "Saltwater", "Type": "MusicAlbum"}]),
        ))
        .mount(&server)
        .await;
    get("/Items")
        .and(query_param("parentId", "al-mv"))
        .respond_with(items(json!([
            song("mv1", "Folk", "Saltwater"),
            song("mv2", "Folk", "Saltwater")
        ])))
        .mount(&server)
        .await;
    get("/Items")
        .and(query_param("searchTerm", "Mara Vey"))
        .respond_with(items(json!([])))
        .mount(&server)
        .await;
    // The rules' candidates: folk, one of them marked Not Interested.
    get("/Items")
        .and(query_param("genreIds", "g-folk"))
        .respond_with(items(json!([
            song("f1", "Folk", "A"),
            song("hidden", "Folk", "B"),
            song("f2", "Folk", "C")
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
                .set_not_interested(true, "hidden", "key-hidden", aoide_store::now_millis())
                .unwrap();
        })
        .await
        .unwrap();
    let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    keyring
        .set(aoide_integrations::openrouter::KEYRING_ENTRY, "sk-or-test")
        .unwrap();
    let client = JellyfinClient::new(
        aoide_jellyfin::ServerAddress::parse(&server.uri()).unwrap(),
        aoide_jellyfin::Device::new("Test", "device-1"),
    )
    .unwrap()
    .with_credentials(aoide_jellyfin::Credentials {
        user_id: "u1".into(),
        access_token: "tok".into(),
        username: "tester".into(),
    });
    let mix = compose(
        OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap(),
        keyring,
        "test/model".into(),
        client,
        store.clone(),
        Some("lib1".into()),
        "forgotten folk and Mara Vey".into(),
    )
    .await
    .unwrap();
    let ids: Vec<&str> = mix.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(&ids[..2], ["mv1", "mv2"], "the names come first: {ids:?}");
    assert_eq!(ids.len(), 4, "cut at the rules' limit: {ids:?}");
    assert!(!ids.contains(&"hidden"), "{ids:?}");
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
