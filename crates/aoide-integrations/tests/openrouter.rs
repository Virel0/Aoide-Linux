//! The OpenRouter client against a local stand-in (wiremock). No network.

use aoide_integrations::openrouter::smart_rules::{Match, RuleField};
use aoide_integrations::openrouter::smart_search::{Ordering, build_prompt};
use aoide_integrations::openrouter::{
    ApiKey, DEFAULT_MODEL, OpenRouter, OpenRouterError, mix_plan, smart_rules,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn key() -> ApiKey {
    ApiKey::new("test-key").unwrap()
}

fn genres() -> Vec<String> {
    ["Rock", "Jazz", "Electronic"].map(String::from).to_vec()
}

fn completion(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "gen-1",
        "choices": [{ "message": { "role": "assistant", "content": content } }],
    }))
}

async fn client(server: &MockServer) -> OpenRouter {
    OpenRouter::with_base_url(&format!("{}/api/v1", server.uri())).unwrap()
}

#[tokio::test]
async fn a_search_sends_only_the_phrase_and_the_genres() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(completion(
            "```json\n{\"genres\":[\"rock\"],\"fromYear\":1990,\"toYear\":1999,\"ordering\":\"newestFirst\"}\n```",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let phrase = "upbeat rock from the nineties";
    let outcome = client(&server)
        .await
        .translate_search(&key(), "some/model", phrase, &genres())
        .await;
    let query = outcome.query.unwrap();
    assert_eq!(outcome.reason, None);
    assert_eq!(query.genres, ["Rock"]);
    assert_eq!((query.from_year, query.to_year), (Some(1990), Some(1999)));
    assert_eq!(query.ordering, Ordering::NewestFirst);

    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "some/model",
            "messages": [{ "role": "user", "content": build_prompt(phrase, &genres()) }],
            "temperature": 0,
        })
    );
    assert_eq!(request.headers["x-title"], "Aoide");
    assert_eq!(
        request.headers["http-referer"],
        "https://github.com/Virel0/aoide-linux"
    );
    let names: Vec<_> = request.headers.keys().map(|k| k.as_str()).collect();
    for name in &names {
        assert!(
            [
                "authorization",
                "content-type",
                "content-length",
                "accept",
                "host",
                "x-title",
                "http-referer"
            ]
            .contains(name),
            "unexpected header {name}"
        );
    }
}

#[tokio::test]
async fn an_empty_model_id_means_the_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion("{}"))
        .mount(&server)
        .await;
    client(&server)
        .await
        .translate_search(&key(), "  ", "quiet jazz for evening", &genres())
        .await;
    let request = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], DEFAULT_MODEL);
}

#[tokio::test]
async fn a_mix_comes_back_as_rules_and_names() {
    let server = MockServer::start().await;
    let reply = json!({
        "match": "all",
        "limit": 40,
        "names": ["Helldivers 2"],
        "rules": [
            { "field": "genre", "op": "is", "value": "rock" },
            { "field": "lastPlayed", "op": "notInTheLast", "value": "-6m" },
            { "field": "title", "op": "inTheLast", "value": "-30d" },
        ],
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .respond_with(completion(&format!("Here you go:\n{reply}")))
        .mount(&server)
        .await;

    let outcome = client(&server)
        .await
        .describe_mix(&key(), "m", "Helldivers 2 session music", &genres())
        .await;
    assert_eq!(outcome.reason, None);
    assert_eq!(outcome.names, ["Helldivers 2"]);
    assert_eq!(outcome.rejected, ["inTheLast does not apply to title"]);
    let rules = outcome.rules.unwrap();
    assert_eq!(rules.match_, Match::All);
    assert_eq!(rules.limit, Some(40));
    assert_eq!(rules.rules[1].field, RuleField::LastPlayed);

    // The genre resolves to the library's own spelling.
    let library = ["Hard Rock", "Rock", "Rockabilly", "Jazz"].map(String::from);
    let matched = mix_plan::match_genres(&mix_plan::described_genres(Some(&rules)), &library);
    assert_eq!(matched, ["Hard Rock", "Rock"]);

    let request = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body["messages"][0]["content"],
        smart_rules::build_mix_prompt("Helldivers 2 session music", &genres())
    );
}

#[tokio::test]
async fn a_refusal_keeps_the_services_own_words_cut_short() {
    let server = MockServer::start().await;
    let long = format!(
        "{{\"error\":{{\"message\":\"Insufficient credits{}\"}}}}",
        "!".repeat(400)
    );
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_string(long.clone()))
        .mount(&server)
        .await;
    let outcome = client(&server)
        .await
        .translate_search(&key(), "m", "a b c", &genres())
        .await;
    assert_eq!(outcome.query, None);
    let reason = outcome.reason.unwrap();
    let expected: String = long.chars().take(200).collect();
    assert_eq!(reason, format!("OpenRouter returned HTTP 402: {expected}"));
}

#[tokio::test]
async fn an_unfamiliar_reply_is_said_so() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "choices": [] })))
        .mount(&server)
        .await;
    let c = client(&server).await;
    assert_eq!(
        c.complete(&key(), "m", "p").await,
        Err(OpenRouterError::UnfamiliarReply)
    );
    let outcome = c.describe_mix(&key(), "m", "calm", &genres()).await;
    assert_eq!(outcome.rules, None);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("OpenRouter returned a reply in an unfamiliar shape.")
    );
}

#[tokio::test]
async fn a_reply_without_json_is_no_query_and_no_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion("I am afraid I cannot help with that."))
        .mount(&server)
        .await;
    let outcome = client(&server)
        .await
        .translate_search(&key(), "m", "a b c", &genres())
        .await;
    assert_eq!((outcome.query, outcome.reason), (None, None));
}

#[tokio::test]
async fn an_unreachable_service_is_a_reason_not_a_panic() {
    // A port nothing listens on: bound, read and closed again. (wiremock
    // pools its servers, so a dropped one still answers.)
    let address = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/api/v1", listener.local_addr().unwrap())
    };
    let c = OpenRouter::with_base_url(&address).unwrap();
    let outcome = c.translate_search(&key(), "m", "a b c", &genres()).await;
    assert!(
        outcome
            .reason
            .unwrap()
            .starts_with("Could not reach OpenRouter")
    );
}

#[tokio::test]
async fn models_are_listed_cheapest_first_without_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "big/model", "name": "Big", "pricing": { "prompt": "0.000003", "completion": "0.000015" }, "context_length": 200000 },
                { "id": "router/auto", "name": "Auto", "pricing": { "prompt": "-1", "completion": "-1" } },
                { "id": "small/model", "pricing": { "prompt": "0.00000015", "completion": "0.0000006" } },
                { "name": "no id" },
                { "id": "free/model", "name": "Free", "pricing": { "prompt": "0", "completion": "0" } },
            ]
        })))
        .mount(&server)
        .await;
    let models = client(&server).await.models().await.unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["free/model", "small/model", "big/model", "router/auto"]
    );
    assert_eq!(models[1].name, "small/model", "the id stands in for a name");
    assert_eq!(models[1].prompt_price, Some(0.000_000_15));
    assert_eq!(models[1].completion_price, Some(0.000_000_6));
    assert_eq!(models[2].context_length, Some(200_000));
    assert_eq!(models[3].prompt_price, None);

    let request = &server.received_requests().await.unwrap()[0];
    assert!(request.headers.get("authorization").is_none());
}

#[tokio::test]
async fn a_failed_model_list_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).set_body_string("down"))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server).await.models().await,
        Err(OpenRouterError::Refused {
            status: 503,
            body: "down".into()
        })
    );
}
