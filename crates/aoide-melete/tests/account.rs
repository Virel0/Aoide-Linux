//! The account's password and two-factor sign-in, and an administrator
//! giving a user a new password, against a local mock server. Nothing here
//! leaves 127.0.0.1, and every password is made at run time ([`password`]).

use aoide_melete::{
    ChangeKind, Credentials, Device, Error, MadeFrom, MeleteClient, ServerAddress, WRONG_CODE,
    WRONG_PASSWORD,
};
use std::hash::{BuildHasher, RandomState};

use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";

fn signed_in(server: &MockServer) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(&server.uri()).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

/// A password made at run time, a different one each call. A password written into
/// the source reads to code scanners as a real one.
fn password() -> String {
    format!("{:016x}", RandomState::new().hash_one(std::process::id()))
}

fn refusal(status: u16, reason: Option<&str>) -> ResponseTemplate {
    let mut body = json!({"error": "refused"});
    if let Some(reason) = reason {
        body["reason"] = json!(reason);
    }
    ResponseTemplate::new(status).set_body_json(body)
}

fn status_and_reason(error: Error) -> (u16, Option<String>) {
    match error {
        Error::Http { status, reason, .. } => (status, reason),
        other => panic!("expected Error::Http, got {other:?}"),
    }
}

#[tokio::test]
async fn a_password_change_sends_both_and_says_each_refusal() {
    let old = password();
    let new = password();
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/me/password"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(json!({
            "current_password": old,
            "new_password": new
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.change_password(&old, &new).await.unwrap();

    for status in [400, 403, 429] {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/me/password"))
            .respond_with(refusal(status, None))
            .mount(&server)
            .await;
        let error = signed_in(&server)
            .change_password(&old, "short")
            .await
            .unwrap_err();
        assert_eq!(status_and_reason(error), (status, None));
    }
}

#[tokio::test]
async fn the_status_reads_each_change() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me/two-factor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "on": true, "on_since": 1_700_000_000,
            "changes": [
                {"change": "wrong_sign_in_code", "made_from": "sign_in", "at": 1_700_000_500, "count": 3},
                {"change": "two_factor_on", "made_from": "account", "at": 1_700_000_000, "count": 1},
                {"change": "two_factor_off", "made_from": "server", "at": 1_600_000_000, "count": 1},
                {"change": "something_new", "made_from": "elsewhere", "at": 1, "count": 1},
            ]
        })))
        .mount(&server)
        .await;
    let status = signed_in(&server).two_factor().await.unwrap();
    assert!(status.on);
    assert_eq!(status.on_since, Some(1_700_000_000));
    let changes: Vec<_> = status
        .changes
        .iter()
        .map(|c| (c.change, c.made_from, c.count))
        .collect();
    assert_eq!(
        changes,
        [
            (ChangeKind::WrongSignInCode, MadeFrom::SignIn, 3),
            (ChangeKind::TwoFactorOn, MadeFrom::Account, 1),
            (ChangeKind::TwoFactorOff, MadeFrom::Server, 1),
            (ChangeKind::Other, MadeFrom::Other, 1),
        ]
    );
}

#[tokio::test]
async fn setting_up_gives_the_secret_and_never_prints_it() {
    let pass = password();
    let wrong = password();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/setup"))
        .and(body_json(json!({"password": pass})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "secret": "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP",
            "uri": "otpauth://totp/Melete:alice?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Melete",
            "qr": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/setup"))
        .and(body_json(json!({"password": wrong})))
        .respond_with(refusal(403, Some(WRONG_PASSWORD)))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let setup = client.start_two_factor(&pass).await.unwrap();
    assert!(setup.uri.starts_with("otpauth://totp/"));
    assert!(setup.qr.as_deref().is_some_and(|q| q.starts_with("<svg")));
    let printed = format!("{setup:?}");
    assert!(
        !printed.contains("JBSWY3DP") && !printed.contains("svg"),
        "{printed}"
    );

    let error = client.start_two_factor(&wrong).await.unwrap_err();
    assert_eq!(
        status_and_reason(error),
        (403, Some(WRONG_PASSWORD.to_owned()))
    );
}

#[tokio::test]
async fn confirming_and_turning_off_send_the_code_and_name_the_wrong_field() {
    let pass = password();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/confirm"))
        .and(body_json(json!({"code": "123456"})))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/confirm"))
        .and(body_json(json!({"code": "000000"})))
        .respond_with(refusal(403, Some(WRONG_CODE)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/off"))
        .and(body_json(json!({"password": pass, "code": "654321"})))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/me/two-factor/off"))
        .and(body_json(json!({"password": pass, "code": "000000"})))
        .respond_with(refusal(429, None))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.confirm_two_factor("123456").await.unwrap();
    let wrong = client.confirm_two_factor("000000").await.unwrap_err();
    assert_eq!(status_and_reason(wrong), (403, Some(WRONG_CODE.to_owned())));
    client.turn_off_two_factor(&pass, "654321").await.unwrap();
    let locked = client
        .turn_off_two_factor(&pass, "000000")
        .await
        .unwrap_err();
    assert_eq!(status_and_reason(locked), (429, None));
}

#[tokio::test]
async fn an_administrator_lists_users_and_gives_one_a_new_password() {
    let made = password();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "username": "alice", "is_admin": true, "avatar_url": null},
            {"id": 2, "username": "robin", "is_admin": false,
             "avatar_url": "/api/v1/users/2/avatar?v=aaaaaaaaaaaa"},
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/2/password"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"username": "robin", "password": made})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/users/1/password"))
        .respond_with(refusal(409, None))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let users = client.users().await.unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!((users[1].id, users[1].is_admin), (2, false));
    let given = client.give_new_password(2).await.unwrap();
    assert_eq!(given.username, "robin");
    assert_eq!(given.password, made);
    assert!(!format!("{given:?}").contains(&made));
    let own = client.give_new_password(1).await.unwrap_err();
    assert_eq!(status_and_reason(own), (409, None));
    assert!(matches!(
        client.give_new_password(9).await,
        Err(Error::NotFound { .. })
    ));
}
