//! Rescan Library: starts the server's "Scan Media Library" task
//! (`refresh_library`). Jellyfin lets administrators only; anyone else is
//! told so, with the server's 403.

use iced::Task;

use crate::app::{App, Message};

#[derive(Debug, Clone)]
pub enum Msg {
    Start,
    Started(Result<(), String>),
}

/// What starting a scan came to, in words.
pub fn outcome(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "The server is scanning the library. Its dashboard shows how far it has got; \
                   new music appears here as it is found."
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
                        Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                        result => Message::Rescan(Msg::Started(result.map_err(|e| {
                            crate::screens::jellyfin_playlist::admin_error(&e, "rescan its library")
                        }))),
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
    use aoide_jellyfin::{Credentials, Device, JellyfinClient, ServerAddress};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_plain_user_is_told_only_an_administrator_can() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ScheduledTasks"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(
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
        let said = outcome(&Err(crate::screens::jellyfin_playlist::admin_error(
            &error,
            "rescan its library",
        )));
        assert!(
            said.contains("Only an administrator of this Jellyfin server can rescan its library"),
            "{said}"
        );
        assert!(outcome(&Ok(())).starts_with("The server is scanning"));
    }
}
