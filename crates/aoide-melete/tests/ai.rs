//! Music by description against a local mock server. Nothing here leaves
//! 127.0.0.1.

use aoide_melete::{
    Credentials, Device, Error, MeleteClient, MusicFilters, MusicOrdering, ServerAddress,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";
const SEARCH: &str = "/api/v1/ai/music/search";
const MIX: &str = "/api/v1/ai/music/mix";

fn signed_in(base: &str) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(base).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

async fn answering(route: &str, status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn whether_ai_is_on_is_read_and_a_way_in_without_it_is_off() {
    for (status, body, expected) in [
        (200, json!({"enabled": true}), true),
        (200, json!({"enabled": false}), false),
        (403, json!({"error": "not through this way in"}), false),
        (404, json!({"error": "no such route"}), false),
    ] {
        let server = answering("/api/v1/ai", status, body).await;
        let client = signed_in(&server.uri());
        assert_eq!(client.ai_enabled().await, Ok(expected), "HTTP {status}");
    }
}

#[tokio::test]
async fn any_other_failure_to_say_is_an_error() {
    let server = answering("/api/v1/ai", 500, json!({"error": "boom"})).await;
    assert!(matches!(
        signed_in(&server.uri()).ai_enabled().await,
        Err(Error::Http { status: 500, .. })
    ));
}

#[tokio::test]
async fn a_description_is_sent_alone_and_its_filters_read() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SEARCH))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(json!({"query": "upbeat rock from the nineties"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "search_term": null, "genres": ["Rock", "Art Rock"], "from_year": 1990,
            "to_year": 1999, "liked_only": true, "ordering": "most_played"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let filters = signed_in(&server.uri())
        .music_filters("upbeat rock from the nineties")
        .await
        .unwrap();
    assert_eq!(
        filters,
        MusicFilters {
            search_term: None,
            genres: vec!["Rock".into(), "Art Rock".into()],
            from_year: Some(1990),
            to_year: Some(1999),
            liked_only: true,
            ordering: MusicOrdering::MostPlayed,
        }
    );
    assert!(!filters.is_empty());
}

#[tokio::test]
async fn every_ordering_is_read_and_an_unknown_one_is_by_name() {
    for (wire, ordering) in [
        ("by_name", MusicOrdering::ByName),
        ("most_played", MusicOrdering::MostPlayed),
        ("newest_first", MusicOrdering::NewestFirst),
        ("random", MusicOrdering::Random),
        ("recently_played", MusicOrdering::RecentlyPlayed),
        ("vibes", MusicOrdering::ByName),
    ] {
        let body = json!({"search_term": "Muse", "genres": [], "from_year": null,
            "to_year": null, "liked_only": false, "ordering": wire});
        let server = answering(SEARCH, 200, body).await;
        let filters = signed_in(&server.uri()).music_filters("x y z").await;
        assert_eq!(filters.unwrap().ordering, ordering, "{wire}");
    }
}

#[tokio::test]
async fn filters_that_narrow_nothing_are_empty() {
    let body = json!({"search_term": null, "genres": [], "from_year": null,
        "to_year": null, "liked_only": false, "ordering": "random"});
    let server = answering(SEARCH, 200, body).await;
    let filters = signed_in(&server.uri()).music_filters("x y z").await;
    assert!(filters.unwrap().is_empty());
}

#[tokio::test]
async fn a_mood_is_sent_alone_and_its_rules_names_and_refusals_read() {
    let server = MockServer::start().await;
    let rules = json!({"match": "all", "limit": null, "rules": [
        {"field": "genre", "op": "is", "value": "rock"},
        {"field": "last_played", "op": "notInTheLast", "value": "-6m"}
    ]});
    Mock::given(method("POST"))
        .and(path(MIX))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(
            json!({"description": "Helldivers 2 session music"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "names": ["Helldivers 2"], "rules": rules,
            "rejected": ["title does not take inTheLast"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mix = signed_in(&server.uri())
        .music_mix("Helldivers 2 session music")
        .await
        .unwrap();
    assert_eq!(mix.names, ["Helldivers 2"]);
    assert_eq!(mix.rules, Some(rules));
    assert_eq!(mix.rejected, ["title does not take inTheLast"]);
}

#[tokio::test]
async fn a_mix_with_no_usable_rules_has_none() {
    let body = json!({"names": [], "rules": null, "rejected": ["no usable rules"]});
    let server = answering(MIX, 200, body).await;
    let mix = signed_in(&server.uri()).music_mix("x y z").await.unwrap();
    assert_eq!(mix.rules, None);
    assert_eq!(mix.rejected, ["no usable rules"]);
}

#[tokio::test]
async fn the_servers_refusals_keep_their_status_and_words() {
    for (status, words) in [
        (503, "AI features aren't set up"),
        (400, "describe what you're after in 1 to 300 characters"),
        (429, "that's 30 searches this hour; try again later"),
        (502, "OpenRouter refused the key"),
    ] {
        for route in [SEARCH, MIX] {
            let server = answering(route, status, json!({"error": words})).await;
            let client = signed_in(&server.uri());
            let failed = if route == SEARCH {
                client.music_filters("x y z").await.map(drop)
            } else {
                client.music_mix("x y z").await.map(drop)
            };
            assert_eq!(
                failed,
                Err(Error::Http {
                    status,
                    path: route.trim_start_matches('/').into(),
                    detail: Some(words.into()),
                }),
                "{route} {status}"
            );
        }
    }
}

#[tokio::test]
async fn a_server_from_before_music_ai_answers_not_found() {
    for route in [SEARCH, MIX] {
        let server = answering(route, 404, json!({"error": "no such route"})).await;
        let client = signed_in(&server.uri());
        let failed = if route == SEARCH {
            client.music_filters("x y z").await.map(drop)
        } else {
            client.music_mix("x y z").await.map(drop)
        };
        assert_eq!(
            failed,
            Err(Error::NotFound {
                path: route.trim_start_matches('/').into()
            }),
            "{route}"
        );
    }
}
