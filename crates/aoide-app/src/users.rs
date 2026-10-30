//! The server's users, for choosing whom to share a playlist with: Melete
//! lists every user's name to anyone signed in.

use aoide_melete::{MeleteClient, UserName};

/// One user, as a share names them.
pub type ServerUser = UserName;

/// What asking for the users found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Users {
    /// The whole list: pick from it.
    Listed(Vec<ServerUser>),
    /// The list couldn't be had: a typed name or id is all there is.
    Typed(Vec<ServerUser>),
}

/// Everyone but `me`, or, when the list can't be had, nobody to pick from
/// (an id can still be typed).
pub async fn users(client: MeleteClient, me: String) -> Users {
    match client.user_directory().await {
        Ok(list) => Users::Listed(list.into_iter().filter(|u| !same_id(&u.id, &me)).collect()),
        Err(_) => Users::Typed(Vec::new()),
    }
}

/// Melete's user ids are numbers, written as text.
pub fn same_id(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

/// Whether `text` is a user id itself: digits only.
pub fn looks_like_id(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
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
    looks_like_id(typed).then(|| typed.to_owned())
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
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn user(id: &str, name: &str) -> ServerUser {
        ServerUser {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn a_typed_name_or_pasted_id_becomes_an_id() {
        let known = [user("7", "Robin")];
        assert_eq!(resolve_typed(" robin ", &known).as_deref(), Some("7"));
        assert_eq!(resolve_typed(" 12 ", &[]).as_deref(), Some("12"));
        assert_eq!(resolve_typed("sam", &known), None);
        assert_eq!(resolve_typed("  ", &known), None);
        assert_eq!(name_of("7", &known), "Robin");
        assert_eq!(name_of("8", &known), "8");
    }

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn everyone_else_is_listed_to_pick_from() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/directory"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 1, "username": "alice"}, {"id": 2, "username": "robin"}
            ])))
            .mount(&server)
            .await;
        assert_eq!(
            users(client(&server), "1".into()).await,
            Users::Listed(vec![user("2", "robin")])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_the_list_an_id_can_still_be_typed() {
        let server = MockServer::start().await;
        assert_eq!(
            users(client(&server), "1".into()).await,
            Users::Typed(Vec::new())
        );
    }
}
