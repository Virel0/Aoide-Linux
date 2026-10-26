//! Mixes: describe a mood, get a queue. The work is the app's
//! (`crate::mixes`, which holds the key); this screen holds the
//! description and the mix while it is looked at, played, kept or thrown
//! away.

use aoide_store::PlaySource;
use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Task};

use super::{Load, PageMsg, View};
use crate::actions::PlayRequest;
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::mixes::{Mix, Request};
use crate::theme;
use crate::widgets::{self, ListStyle};

pub const FIELD: &str = "mixes-field";

#[derive(Debug, Default)]
pub struct State {
    description: String,
    /// The description asked about, and what came of it.
    mix: Option<(String, Load<Box<Mix>>)>,
    /// What saving said: the playlist's name, or why not.
    saved: Option<Result<String, String>>,
    saving: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Typed(String),
    Compose,
    Composed(String, Result<Box<Mix>, crate::failure::Failure>),
    /// Compose the description that failed again.
    Retry,
    Save,
    Saved(Result<String, String>),
    Discard,
}

pub fn open() -> (State, Task<App>) {
    (State::default(), iced::widget::operation::focus(FIELD))
}

impl State {
    pub fn update(&mut self, message: Message, key: u64) -> Task<App> {
        match message {
            Message::Typed(description) => {
                self.description = description;
                Task::none()
            }
            Message::Compose => {
                let description = self.description.trim().to_owned();
                if description.is_empty() || self.is_loading() {
                    return Task::none();
                }
                self.mix = Some((description.clone(), Load::Loading));
                self.saved = None;
                Task::done(App::Mixes(Request::Compose {
                    page: key,
                    description,
                }))
            }
            Message::Composed(description, result) => {
                if self.mix.as_ref().is_some_and(|(d, _)| *d == description) {
                    self.mix = Some((description, Load::from(result)));
                }
                Task::none()
            }
            Message::Retry => match self.mix.take() {
                Some((description, Load::Failed(_))) => {
                    self.description = description;
                    self.update(Message::Compose, key)
                }
                other => {
                    self.mix = other;
                    Task::none()
                }
            },
            Message::Save => {
                let Some(mix) = self.ready().filter(|m| m.can_save()).cloned() else {
                    return Task::none();
                };
                self.saving = true;
                Task::done(App::Mixes(Request::Save {
                    page: key,
                    mix: Box::new(mix),
                }))
            }
            Message::Saved(result) => {
                self.saving = false;
                self.saved = Some(result);
                Task::none()
            }
            Message::Discard => {
                self.mix = None;
                self.saved = None;
                iced::widget::operation::focus(FIELD)
            }
        }
    }

    fn is_loading(&self) -> bool {
        matches!(self.mix, Some((_, Load::Loading)))
    }

    fn ready(&self) -> Option<&Mix> {
        match &self.mix {
            Some((_, Load::Ready(mix))) => Some(mix),
            _ => None,
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        self.ready()
            .map(|m| super::track_art(&m.tracks))
            .unwrap_or_default()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let send = move |m: Message| App::Page(key, PageMsg::Mixes(m));
        let mut body = column![widgets::title_bar("Mixes", view.can_go_back)].spacing(20);
        if !view.smart_search {
            return widgets::page(
                body.push(
                    column![
                        text("Mixes need an OpenRouter key."),
                        widgets::secondary(
                            "A language model turns your description into rules; your library \
                             and your listening history pick the songs. Add a key in Settings → \
                             Smart search."
                        )
                        .size(13),
                        button(text("Open settings").size(13))
                            .style(theme::tonal)
                            .padding([6, 12])
                            .on_press(App::OpenSettings(super::settings::Category::SmartSearch,)),
                    ]
                    .spacing(10),
                ),
            );
        }
        let busy = self.is_loading();
        let can_make = !busy && !self.description.trim().is_empty();
        body = body.push(
            row![
                text_input(
                    "Describe a mood: rainy Sunday folk, forgotten favourites, music for a long drive",
                    &self.description,
                )
                .id(FIELD)
                .on_input(move |s| send(Message::Typed(s)))
                .on_submit_maybe(can_make.then(|| send(Message::Compose)))
                .padding(10)
                .size(16)
                .style(theme::field),
                button(text(if busy { "Mixing…" } else { "Make a Mix" }).size(14))
                    .style(theme::filled)
                    .padding([10, 16])
                    .on_press_maybe(can_make.then(|| send(Message::Compose))),
            ]
            .spacing(10)
            .align_y(Center),
        );
        body = body.push(
            widgets::secondary(
                "Only your description and your library's genre names go to OpenRouter. \
                 Artists and albums you name are looked up on your server.",
            )
            .size(12),
        );
        match self.mix.as_ref().map(|(_, mix)| mix) {
            None => {}
            Some(Load::Loading) => body = body.push(widgets::loading()),
            Some(Load::Failed(error)) => {
                body = body.push(widgets::failure(
                    error,
                    Some(App::Page(key, PageMsg::Mixes(Message::Retry))),
                    view.details,
                ));
            }
            Some(Load::Ready(mix)) => body = body.push(self.mix_view(mix, view)),
        }
        widgets::page(body)
    }

    fn mix_view<'a>(&'a self, mix: &'a Mix, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut part =
            column![widgets::secondary(format!("Looked for: {}", mix.looked_for)).size(13)]
                .spacing(12);
        if let Some(missed) = &mix.missed {
            part = part.push(widgets::secondary(missed.as_str()).size(13));
        }
        if !mix.rejected.is_empty() {
            part = part.push(
                widgets::secondary(format!("Left out: {}.", mix.rejected.join("; "))).size(13),
            );
        }
        part = part.push(self.mix_actions(mix, key));
        match &self.saved {
            Some(Ok(name)) => {
                part = part.push(
                    widgets::secondary(format!(
                        "Saved as “{name}”, a smart playlist that keeps these rules and fills \
                         itself from your library."
                    ))
                    .size(13),
                );
            }
            Some(Err(error)) => {
                part = part.push(text(format!("The playlist couldn't be saved: {error}")).size(13));
            }
            None if !mix.can_save() && !mix.tracks.is_empty() => {
                part = part.push(
                    widgets::secondary(
                        "This mix is made of the names you gave, which a smart playlist's rules \
                         can't hold, so it can be played but not kept.",
                    )
                    .size(12),
                );
            }
            None => {}
        }
        if mix.tracks.is_empty() {
            part = part.push(widgets::empty(
                "Nothing in your library fits that description.",
            ));
        } else {
            part = part.push(widgets::track_list(
                &mix.tracks,
                view,
                ListStyle {
                    numbered: false,
                    show_album: true,
                },
                PlaySource::Smart,
            ));
        }
        part.into()
    }

    /// Play, Save as Smart Playlist (when the rules can be kept and it isn't
    /// saved yet) and Discard.
    fn mix_actions<'a>(&self, mix: &'a Mix, key: u64) -> iced::widget::Row<'a, App> {
        let send = move |m: Message| App::Page(key, PageMsg::Mixes(m));
        let mut actions = row![].spacing(8).align_y(Center);
        if !mix.tracks.is_empty() {
            actions = actions.push(
                button(text("Play").size(14))
                    .style(theme::filled)
                    .padding([6, 16])
                    .on_press(App::Play(PlayRequest::from(
                        &mix.tracks,
                        0,
                        PlaySource::Smart,
                    ))),
            );
        }
        let save = match (&self.saved, mix.can_save()) {
            (Some(Ok(_)), _) => None,
            (_, true) => Some(
                button(
                    text(if self.saving {
                        "Saving…"
                    } else {
                        "Save as smart playlist"
                    })
                    .size(14),
                )
                .style(theme::tonal)
                .padding([6, 14])
                .on_press_maybe((!self.saving).then(|| send(Message::Save))),
            ),
            (_, false) => None,
        };
        if let Some(save) = save {
            actions = actions.push(save);
        }
        actions.push(
            button(text("Discard").size(14))
                .style(theme::tonal)
                .padding([6, 14])
                .on_press(send(Message::Discard)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(description: &str) -> Box<Mix> {
        Box::new(Mix {
            description: description.into(),
            looked_for: "Folk".into(),
            tracks: Vec::new(),
            rules: None,
            rejected: Vec::new(),
            missed: None,
        })
    }

    #[test]
    fn a_description_is_composed_once_and_a_stale_answer_dropped() {
        let mut state = State::default();
        let _ = state.update(Message::Compose, 1);
        assert!(state.mix.is_none(), "an empty description asks nothing");
        let _ = state.update(Message::Typed("rainy folk".into()), 1);
        let _ = state.update(Message::Compose, 1);
        assert!(state.is_loading());
        // A second press while mixing does nothing.
        let _ = state.update(Message::Compose, 1);
        let _ = state.update(Message::Composed("older".into(), Ok(mix("older"))), 1);
        assert!(state.is_loading());
        let _ = state.update(
            Message::Composed("rainy folk".into(), Ok(mix("rainy folk"))),
            1,
        );
        assert_eq!(state.ready().unwrap().description, "rainy folk");
        let _ = state.update(Message::Discard, 1);
        assert!(state.mix.is_none());
    }

    #[test]
    fn a_mix_without_rules_is_not_saved() {
        let mut state = State {
            mix: Some(("x".into(), Load::Ready(mix("x")))),
            ..State::default()
        };
        let _ = state.update(Message::Save, 1);
        assert!(!state.saving);
    }
}
