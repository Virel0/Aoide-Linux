//! Search by description and Mixes go through the server's AI: Melete turns
//! the words into filters or rules with its own OpenRouter key, and the
//! library picks the songs. This computer holds no key and talks to no AI
//! service.
//!
//! Whether to offer either is asked at sign-in (`/api/v1/ai`) and learnt
//! from the answers: a 503 is a server with AI off, a 404 one without music
//! AI yet. Either holds for the session, so the next sign-in or launch asks
//! again.
//!
//! Search by description ([`search_by_description`]) sends the phrase only
//! when the listener presses the button, never on a keystroke; the filters
//! then go to Melete as an ordinary query, so the songs are always the
//! server's.

use aoide_melete::MeleteClient;
use iced::Task;

use crate::app::{App, Message as AppMessage};

mod describe;
mod grounding;

pub use describe::{Described, Failure};

use describe::search_by_description;

/// Whether the server's music AI is offered this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ai {
    /// On, or the server couldn't say: offered, and its answer decides.
    #[default]
    Offered,
    /// Off on the server, or out of reach through this way in.
    Off,
    /// The server has no music AI yet: a Melete from before it.
    Missing,
}

impl Ai {
    pub fn offered(self) -> bool {
        self == Self::Offered
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// What the server said of its AI, at sign-in or in an answer.
    Learnt(Ai),
    /// Search's "Search by description", for the page with this key.
    Describe { page: u64, phrase: String },
}

/// Asks whether the server's AI is on, at sign-in. A failure to say leaves
/// it offered, for the first answer to decide.
pub fn ask(client: MeleteClient) -> Task<AppMessage> {
    Task::perform(
        async move { client.ai_enabled().await },
        |answer| match answer {
            Ok(true) => AppMessage::Ignore,
            Ok(false) => AppMessage::Smart(Message::Learnt(Ai::Off)),
            Err(error) => {
                log::warn!("The server didn't say whether its AI is on: {error}");
                AppMessage::Ignore
            }
        },
    )
}

/// An answer that used the server's AI, delivered with `page`: a rejected
/// token signs out, and a server without the AI or with it off is
/// remembered for the session before the page hears of it.
pub fn deliver<T>(
    result: Result<T, Failure>,
    page: impl FnOnce(Result<T, crate::failure::Failure>) -> AppMessage,
) -> Task<AppMessage> {
    let failure = match result {
        Ok(found) => return Task::done(page(Ok(found))),
        Err(failure) => failure,
    };
    if failure.is_unauthorized() {
        return Task::done(AppMessage::TokenRejected);
    }
    let learnt = failure
        .availability()
        .map(|ai| Task::done(AppMessage::Smart(Message::Learnt(ai))));
    let shown = Task::done(page(Err(failure.into())));
    Task::batch(learnt.into_iter().chain([shown]))
}

impl App {
    pub(crate) fn smart_message(&mut self, message: Message) -> Task<AppMessage> {
        match message {
            Message::Learnt(ai) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.ai = ai;
                }
                Task::none()
            }
            Message::Describe { page, phrase } => self.describe(page, phrase),
        }
    }

    /// Search by description for the search page with key `page`.
    fn describe(&mut self, page: u64, phrase: String) -> Task<AppMessage> {
        use crate::screens::{PageMsg, search};
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let asked = phrase.clone();
        Task::perform(
            search_by_description(signed.connection.client.clone(), self.store.clone(), phrase),
            std::convert::identity,
        )
        .then(move |result| {
            let asked = asked.clone();
            deliver(result, move |result| {
                AppMessage::Page(
                    page,
                    PageMsg::Search(search::Message::Described(asked, result)),
                )
            })
        })
    }
}
