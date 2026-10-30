//! Rescan Library: asks Melete to scan every library whole
//! (`refresh_library`). Melete lets administrators only; anyone else is told
//! so, with the server's 403.

use iced::Task;

use crate::app::{App, Message};

#[derive(Debug, Clone)]
pub enum Msg {
    Start,
    Started(Result<(), String>),
}

/// An error from an administrators-only call, in words: a 403 says who may.
fn admin_error(error: &aoide_melete::Error, what: &str) -> String {
    match error {
        aoide_melete::Error::Http { status: 403, .. } => format!(
            "Only an administrator of this Melete server can {what}. The server refused \
             (HTTP 403 Forbidden)."
        ),
        other => crate::failure::describe(other).to_string(),
    }
}

/// What starting a scan came to, in words.
pub fn outcome(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "The server is scanning the library. Melete's web UI shows how far it has \
                   got; new music appears here as it is found."
            .to_owned(),
        Err(error) => format!("The library couldn't be rescanned. {error}"),
    }
}

impl App {
    pub(crate) fn rescan(&mut self, message: Msg) -> Task<Message> {
        match message {
            Msg::Start => {
                let Some(signed) = self.session.as_ref() else {
                    return Task::none();
                };
                let client = signed.connection.client.clone();
                Task::perform(
                    async move { client.refresh_library().await },
                    |result| match result {
                        Err(aoide_melete::Error::Unauthorized) => Message::TokenRejected,
                        result => Message::Rescan(Msg::Started(
                            result.map_err(|e| admin_error(&e, "rescan its library")),
                        )),
                    },
                )
            }
            Msg::Started(result) => {
                self.notice = Some(outcome(&result));
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{Credentials, Device, MeleteClient, ServerAddress};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_plain_user_is_told_only_an_administrator_can() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/admin/system/tasks/library/run"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let client = MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let error = client.refresh_library().await.unwrap_err();
        let said = outcome(&Err(admin_error(&error, "rescan its library")));
        assert!(
            said.contains("Only an administrator of this Melete server can rescan its library"),
            "{said}"
        );
        assert!(outcome(&Ok(())).starts_with("The server is scanning"));
    }
}
