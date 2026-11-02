//! Motion covers against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    Credentials, Device, MOTION_COVER_MAX_BYTES, MeleteClient, MotionCoverChoice, ServerAddress,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";
const MOTION: &str = "/api/v1/music/albums/2/motion";

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

#[tokio::test]
async fn a_motion_cover_is_read_with_the_token() {
    let server = MockServer::start().await;
    let video = b"\0\0\0\x18ftypisom-not-really-a-video".to_vec();
    Mock::given(method("GET"))
        .and(path(MOTION))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "video/mp4")
                .set_body_bytes(video.clone()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let read = signed_in(&server.uri()).motion_cover(MOTION).await.unwrap();
    assert_eq!(read, Some(video));
}

#[tokio::test]
async fn a_motion_cover_is_read_beneath_a_proxy_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/melete{MOTION}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3]))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&format!("{}/melete", server.uri()));
    assert_eq!(
        client.motion_cover(MOTION).await.unwrap(),
        Some(vec![1, 2, 3])
    );
}

#[tokio::test]
async fn a_path_that_isnt_the_servers_own_is_never_asked_for() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1]))
        .expect(0)
        .mount(&server)
        .await;
    let client = signed_in(&server.uri());
    for path in [
        "https://elsewhere.example/video.mp4",
        "//elsewhere.example/video.mp4",
        "api/v1/music/albums/2/motion",
        "/api/v1/music/albums/../../motion",
        "/api/v1/music/albums/2/motion?x=1",
    ] {
        assert_eq!(client.motion_cover(path).await.unwrap(), None, "{path}");
    }
}

#[tokio::test]
async fn a_video_past_the_limit_or_none_for_this_user_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MOTION))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![0u8; MOTION_COVER_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums/3/motion"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "no motion cover"})))
        .mount(&server)
        .await;
    let client = signed_in(&server.uri());
    assert_eq!(client.motion_cover(MOTION).await.unwrap(), None);
    assert_eq!(
        client
            .motion_cover("/api/v1/music/albums/3/motion")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_failed_read_is_an_error_not_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MOTION))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    assert!(signed_in(&server.uri()).motion_cover(MOTION).await.is_err());
}

#[tokio::test]
async fn the_choice_is_read_and_set_for_this_user() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/motion-covers"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"show_generated": true, "on": true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/music/motion-covers"))
        .and(body_json(json!({"show_generated": false})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"show_generated": false, "on": true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server.uri());
    assert_eq!(
        client.motion_cover_choice().await.unwrap(),
        MotionCoverChoice {
            show_generated: true,
            on: true
        }
    );
    assert_eq!(
        client
            .set_show_generated_motion_covers(false)
            .await
            .unwrap(),
        MotionCoverChoice {
            show_generated: false,
            on: true
        }
    );
}
