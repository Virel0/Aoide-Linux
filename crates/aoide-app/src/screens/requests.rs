//! Requests: the music this listener has asked Melete for, newest first, with
//! where each stands (waiting, declined and why, or how its download is going)
//! and Withdraw for one still waiting. Asking happens in Search.

use aoide_melete::{MusicRequest, RequestKind, RequestState};
use iced::widget::{button, column, row, text};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, Load, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::failure::Failure;
use crate::{theme, widgets};

#[derive(Debug)]
pub struct State {
    requests: Load<Vec<MusicRequest>>,
    /// A withdrawal on its way, by request id.
    withdrawing: Option<i64>,
    /// Why the last withdrawal failed.
    problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<MusicRequest>, Failure>),
    Refresh,
    Withdraw(i64),
    Withdrawn(Result<(), Failure>),
}

fn load(ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    fetch(ctx.key, async move { client.music_requests().await }, |r| {
        PageMsg::Requests(Message::Loaded(r))
    })
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let state = State {
        requests: Load::Loading,
        withdrawing: None,
        problem: None,
    };
    (state, load(ctx))
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                self.requests = Load::from(result);
                Task::none()
            }
            Message::Refresh => load(ctx),
            Message::Withdraw(id) => {
                self.withdrawing = Some(id);
                self.problem = None;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.withdraw_request(id).await },
                    |r| PageMsg::Requests(Message::Withdrawn(r)),
                )
            }
            Message::Withdrawn(result) => {
                self.withdrawing = None;
                if let Err(error) = result {
                    self.problem = Some(error.to_string());
                }
                load(ctx)
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let refresh = button(text("Refresh").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press(App::Page(key, PageMsg::Requests(Message::Refresh)));
        let mut body = column![
            widgets::title_bar("Requests", view.can_go_back),
            row![
                widgets::secondary("Music you've asked your server for. Ask for more from Search.")
                    .size(13)
                    .width(Fill),
                refresh,
            ]
            .align_y(Center),
        ]
        .spacing(16);
        if let Some(problem) = &self.problem {
            body = body.push(widgets::secondary(problem.as_str()).size(13));
        }
        body = body.push(match &self.requests {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(requests) if requests.is_empty() => widgets::empty(
                "Nothing asked for yet. Search for an artist, album or song that isn't in your library, then ask for it.",
            ),
            Load::Ready(requests) => column(requests.iter().map(|r| self.row(r, key)))
                .spacing(10)
                .into(),
        });
        widgets::page(body)
    }

    /// One request: what it is, and where it stands.
    fn row<'a>(&self, request: &'a MusicRequest, key: u64) -> Element<'a, App> {
        let mut what = vec![kind_word(request.kind).to_owned()];
        if let Some(artist) = &request.artist {
            what.push(artist.clone());
        }
        if let Some(year) = request.year {
            what.push(year.to_string());
        }
        let mut line = row![
            column![
                text(request.title.as_str())
                    .size(14)
                    .font(widgets::SEMIBOLD),
                widgets::secondary(what.join(" · ")).size(12),
                text(standing(request)).size(12).style(|t| text::Style {
                    color: Some(theme::accent(t)),
                }),
            ]
            .spacing(2)
            .width(Fill)
        ]
        .align_y(Center);
        if request.state == RequestState::Requested {
            let busy = self.withdrawing == Some(request.id);
            line = line.push(
                button(text(if busy { "Withdrawing…" } else { "Withdraw" }).size(12))
                    .style(theme::tonal)
                    .padding([4, 10])
                    .on_press_maybe((!busy).then_some(App::Page(
                        key,
                        PageMsg::Requests(Message::Withdraw(request.id)),
                    ))),
            );
        }
        line.into()
    }
}

fn kind_word(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Artist => "Artist",
        RequestKind::Album => "Album",
        RequestKind::Song => "Song",
        RequestKind::Other => "Request",
    }
}

/// Where a request stands, in words.
pub fn standing(request: &MusicRequest) -> String {
    match request.state {
        RequestState::Requested => "Waiting for approval".to_owned(),
        RequestState::Declined => match &request.reason {
            Some(reason) => format!("Declined: {reason}"),
            None => "Declined".to_owned(),
        },
        RequestState::Approved => match &request.progress {
            None => "Approved".to_owned(),
            Some(progress) => match (progress.step.as_str(), progress.percent) {
                ("downloading", Some(percent)) => format!("Downloading, {percent:.0}%"),
                ("wanted", _) => "Approved, waiting to be looked for".to_owned(),
                ("searching", _) => "Looking for a download".to_owned(),
                ("downloading", None) => "Downloading".to_owned(),
                ("transferring", _) => "Copying to your server".to_owned(),
                ("importing", _) => "Adding to your library".to_owned(),
                ("in_library", _) => "In your library".to_owned(),
                ("failed", _) => progress.message.clone().map_or_else(
                    || "Couldn't be fetched".to_owned(),
                    |m| format!("Couldn't be fetched: {m}"),
                ),
                (other, _) => progress.message.clone().unwrap_or_else(|| other.to_owned()),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::Progress;

    fn request(state: RequestState, progress: Option<(&str, Option<f64>)>) -> MusicRequest {
        MusicRequest {
            id: 1,
            kind: RequestKind::Album,
            title: "Low Tide".into(),
            year: None,
            artist: None,
            state,
            reason: (state == RequestState::Declined).then(|| "not on any tracker".into()),
            created_at: 0,
            progress: progress.map(|(step, percent)| Progress {
                step: step.into(),
                percent,
                message: Some("No release fits the profile".into()),
            }),
        }
    }

    #[test]
    fn a_request_says_where_it_stands() {
        assert_eq!(
            standing(&request(RequestState::Requested, None)),
            "Waiting for approval"
        );
        assert_eq!(
            standing(&request(RequestState::Declined, None)),
            "Declined: not on any tracker"
        );
        assert_eq!(
            standing(&request(
                RequestState::Approved,
                Some(("downloading", Some(42.4)))
            )),
            "Downloading, 42%"
        );
        assert_eq!(
            standing(&request(RequestState::Approved, Some(("in_library", None)))),
            "In your library"
        );
        assert_eq!(
            standing(&request(RequestState::Approved, Some(("failed", None)))),
            "Couldn't be fetched: No release fits the profile"
        );
    }
}
