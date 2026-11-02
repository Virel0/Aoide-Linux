//! The Requests page kept up to date: Melete's request feed while the page is
//! open, opened again when it ends (it does every 10 minutes); and, while a
//! playlist is still being asked for, its tally every few seconds.

use std::hash::{Hash, Hasher};
use std::time::Duration;

use aoide_melete::MeleteClient;
use iced::Subscription;
use iced::futures::{SinkExt, Stream};

use super::Message;
use crate::app::Message as App;
use crate::screens::{Page, PageMsg};

/// How long to wait before opening the feed again after it failed.
const RETRY: Duration = Duration::from_secs(15);
/// How often a playlist still being asked for is asked about.
const IMPORT_POLL: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Feed {
    client: MeleteClient,
    key: u64,
}

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "aoide-requests-feed".hash(state);
        self.key.hash(state);
    }
}

fn stream_feed(feed: &Feed) -> impl Stream<Item = App> + use<> {
    let (client, key) = (feed.client.clone(), feed.key);
    iced::stream::channel(4, async move |mut output| {
        loop {
            match client.follow_requests().await {
                Ok(mut live) => {
                    while let Ok(Some(requests)) = live.next().await {
                        let message = App::Page(key, PageMsg::Requests(Message::Live(requests)));
                        if output.send(message).await.is_err() {
                            return;
                        }
                    }
                }
                Err(aoide_melete::Error::Unauthorized) => {
                    let _ = output.send(App::TokenRejected).await;
                    return;
                }
                Err(_) => tokio::time::sleep(RETRY).await,
            }
        }
    })
}

impl crate::app::App {
    /// The feed while the Requests page is open.
    pub(crate) fn requests_subscription(&self) -> Subscription<App> {
        let Some(signed) = self.session.as_ref() else {
            return Subscription::none();
        };
        let Some(entry) = signed.nav.as_ref().map(|nav| nav.current()) else {
            return Subscription::none();
        };
        let Page::Requests(state) = &entry.page else {
            return Subscription::none();
        };
        let key = entry.key;
        let feed = Subscription::run_with(
            Feed {
                client: signed.connection.client.clone(),
                key,
            },
            stream_feed,
        );
        if state.imports_running() {
            let poll = iced::time::every(IMPORT_POLL)
                .with(key)
                .map(|(key, _)| App::Page(key, PageMsg::Requests(Message::RefreshImports)));
            Subscription::batch([feed, poll])
        } else {
            feed
        }
    }
}
