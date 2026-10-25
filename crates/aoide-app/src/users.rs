//! The server's users, for choosing whom to share a playlist with.
//!
//! aoide-jellyfin has no call for this, so the two reads are made here with
//! the client's own address, device header and token: `GET /Users` (the whole
//! list, which Jellyfin gives administrators) and, when that is refused,
//! `GET /Users/Public` (those shown on the sign-in screen), which is enough to
//! turn a typed username into the id the sidecar needs.

use std::sync::LazyLock;
use std::time::Duration;

use aoide_jellyfin::JellyfinClient;
use serde::Deserialize;

/// One user, as a share names them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerUser {
    pub id: String,
    pub name: String,
}

/// What asking for the users found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Users {
    /// The whole list: pick from it.
    Listed(Vec<ServerUser>),
    /// Listing refused; these are the ones the sign-in screen shows, which a
    /// typed name is looked up among.
    Typed(Vec<ServerUser>),
}

static HTTP: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .ok()
});

async fn get(client: &JellyfinClient, segments: &[&str]) -> Result<Vec<ServerUser>, u16> {
    let http = HTTP.as_ref().ok_or(0u16)?;
    let mut url = client.address().as_url().clone();
    if let Ok(mut path) = url.path_segments_mut() {
        path.pop_if_empty().extend(segments);
    }
    let token = client.credentials().map(|c| c.access_token.as_str());
    let response = http
        .get(url)
        .header("Authorization", client.device().authorization_header(token))
        .send()
        .await
        .map_err(|_| 0u16)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(status);
    }
    response.json().await.map_err(|_| status)
}

/// Everyone but `me`: the whole list where the server allows it, else the
/// sign-in screen's users to look a typed name up among.
pub async fn users(client: JellyfinClient, me: String) -> Users {
    let others = |list: Vec<ServerUser>| -> Vec<ServerUser> {
        list.into_iter().filter(|u| !same_id(&u.id, &me)).collect()
    };
    match get(&client, &["Users"]).await {
        Ok(list) => Users::Listed(others(list)),
        Err(_) => Users::Typed(others(
            get(&client, &["Users", "Public"]).await.unwrap_or_default(),
        )),
    }
}

/// Jellyfin spells a user id with or without hyphens, in either case.
pub fn same_id(a: &str, b: &str) -> bool {
    let plain = |s: &str| s.replace('-', "").to_lowercase();
    plain(a) == plain(b)
}

/// Whether `text` is a user id itself: 32 hex digits, hyphens allowed.
pub fn looks_like_id(text: &str) -> bool {
    let plain = text.trim().replace('-', "");
    plain.len() == 32 && plain.chars().all(|c| c.is_ascii_hexdigit())
}

/// The id a typed username (or pasted id) stands for: an exact name, ignoring
/// case, among `known`, or the id as pasted.
pub fn resolve_typed(typed: &str, known: &[ServerUser]) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    if let Some(user) = known.iter().find(|u| u.name.eq_ignore_ascii_case(typed)) {
        return Some(user.id.clone());
    }
    looks_like_id(typed).then(|| typed.replace('-', "").to_lowercase())
}

/// A user's name where it is known, else their id.
pub fn name_of<'a>(id: &'a str, known: &'a [ServerUser]) -> &'a str {
    known
        .iter()
        .find(|u| same_id(&u.id, id))
        .map_or(id, |u| u.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{header_regex, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn user(id: &str, name: &str) -> ServerUser {
        ServerUser {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn a_typed_name_or_pasted_id_becomes_an_id() {
        let known = [user("0f8e5a2b9c4d4e1fa3b2c1d0e9f8a7b6", "Robin")];
        assert_eq!(
            resolve_typed(" robin ", &known).as_deref(),
            Some("0f8e5a2b9c4d4e1fa3b2c1d0e9f8a7b6")
        );
        assert_eq!(
            resolve_typed("1A2B3C4D-5E6F-4708-9A0B-1C2D3E4F5A6B", &[]).as_deref(),
            Some("1a2b3c4d5e6f47089a0b1c2d3e4f5a6b")
        );
        assert_eq!(resolve_typed("sam", &known), None);
        assert_eq!(resolve_typed("  ", &known), None);
        assert_eq!(
            name_of("0F8E5A2B-9C4D-4E1F-A3B2-C1D0E9F8A7B6", &known),
            "Robin"
        );
        assert_eq!(name_of("other", &known), "other");
    }

    fn client(server: &MockServer) -> JellyfinClient {
        JellyfinClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "me".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_administrator_picks_from_everyone_else() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .and(header_regex("Authorization", r#"Token="tok""#))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"Id": "me", "Name": "alice"}, {"Id": "u2", "Name": "robin"}
            ])))
            .mount(&server)
            .await;
        assert_eq!(
            users(client(&server), "me".into()).await,
            Users::Listed(vec![user("u2", "robin")])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn anyone_else_types_a_name_looked_up_among_the_public_users() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Users/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"Id": "u2", "Name": "robin"}
            ])))
            .mount(&server)
            .await;
        assert_eq!(
            users(client(&server), "me".into()).await,
            Users::Typed(vec![user("u2", "robin")])
        );
    }
}
