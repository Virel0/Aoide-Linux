//! Merge into…: this playlist's songs added to the end of another hand-made
//! playlist (`Store::merge_playlist`), then the choice to delete this one,
//! which is kept unless that is chosen.

use aoide_store::MergeReport;
use iced::widget::{Column, button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Message, State};
use crate::app::Message as App;
use crate::screens::{Ctx, PageMsg, PlaylistSummary, View};
use crate::{actions, theme, widgets};

/// Where a merge stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Merge {
    #[default]
    Closed,
    /// Choosing the playlist to merge into, found by name once there are many.
    Choosing { filter: String },
    /// The songs are being added.
    Merging,
    /// Done: asking whether to delete this playlist now.
    Merged,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Open,
    Cancel,
    Filter(String),
    Into { id: String, name: String },
    Merged(Result<(String, MergeReport), String>),
    Keep,
    Delete,
}

/// The playlists this one can be merged into: every hand-made one but itself.
pub fn targets<'a>(playlists: &'a [PlaylistSummary], source: &str) -> Vec<&'a PlaylistSummary> {
    playlists
        .iter()
        .filter(|p| !p.smart && p.id != source)
        .collect()
}

/// What a merge into `into` did, for the notice.
pub fn said(into: &str, report: MergeReport) -> String {
    match report.added() {
        0 => format!("Nothing new to add to \u{201c}{into}\u{201d}."),
        1 => format!("Added 1 song to \u{201c}{into}\u{201d}."),
        n => format!("Added {n} songs to \u{201c}{into}\u{201d}."),
    }
}

impl State {
    pub(super) fn merge(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Open => {
                self.merge = Merge::Choosing {
                    filter: String::new(),
                }
            }
            Msg::Filter(typed) => {
                if let Merge::Choosing { filter } = &mut self.merge {
                    *filter = typed;
                }
            }
            Msg::Cancel | Msg::Keep => self.merge = Merge::Closed,
            Msg::Into { id, name } => return self.merge_into(id, name, ctx),
            Msg::Merged(Ok((into, report))) => {
                self.merge = Merge::Merged;
                return Task::batch([
                    Task::done(App::Notice(said(&into, report))),
                    Task::done(App::PlaylistsChanged),
                ]);
            }
            Msg::Merged(Err(error)) => {
                self.merge = Merge::Closed;
                return Task::done(App::Notice(format!(
                    "The playlists couldn't be merged: {error}"
                )));
            }
            Msg::Delete => {
                self.merge = Merge::Closed;
                return self.update(Message::ConfirmDelete(true), ctx);
            }
        }
        Task::none()
    }

    /// Adds this playlist's songs to the playlist `id`, named `name`.
    fn merge_into(&mut self, id: String, name: String, ctx: &Ctx<'_>) -> Task<App> {
        if self.merge == Merge::Merging {
            return Task::none();
        }
        self.merge = Merge::Merging;
        let (store, source, key) = (ctx.store.clone(), self.id.clone(), ctx.key);
        Task::perform(
            async move {
                let now = aoide_store::now_millis();
                let local = aoide_store::Calendar::Local;
                store
                    .call(move |w| w.store.merge_playlist(&source, &id, now, local))
                    .await
                    .ok_or_else(|| "the library is closing".to_owned())?
                    .map(|report| (name, report))
                    .map_err(|e| e.to_string())
            },
            move |r| App::Page(key, PageMsg::Playlist(Message::Merge(Msg::Merged(r)))),
        )
    }

    /// The picker, the merge under way, or whether to delete this playlist.
    pub(super) fn merge_view<'a>(&'a self, view: &View<'a>) -> Option<Element<'a, App>> {
        let key = view.key;
        let message = move |m: Msg| App::Page(key, PageMsg::Playlist(Message::Merge(m)));
        match &self.merge {
            Merge::Closed => None,
            Merge::Choosing { filter } => Some(self.choices(filter, view)),
            Merge::Merging => Some(widgets::secondary("Merging…").into()),
            Merge::Merged => Some(
                row![
                    text(format!("Delete \u{201c}{}\u{201d}?", self.name())),
                    button("Keep")
                        .style(theme::tonal)
                        .padding([8, 14])
                        .on_press(message(Msg::Keep)),
                    button("Delete")
                        .style(theme::destructive)
                        .padding([8, 14])
                        .on_press(message(Msg::Delete)),
                ]
                .spacing(8)
                .align_y(Center)
                .wrap()
                .vertical_spacing(8)
                .into(),
            ),
        }
    }

    /// The hand-made playlists to merge into, with Cancel.
    fn choices<'a>(&'a self, filter: &'a str, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Msg| App::Page(key, PageMsg::Playlist(Message::Merge(m)));
        let others = targets(view.playlists, &self.id);
        let mut list = Column::new()
            .spacing(2)
            .push(widgets::secondary("Merge into").size(12));
        if others.len() > actions::PLAYLIST_SEARCH_AFTER {
            list = list.push(
                text_input("Find a playlist", filter)
                    .on_input(move |s| message(Msg::Filter(s)))
                    .padding(6)
                    .size(13)
                    .style(theme::field),
            );
        }
        let shown = actions::matching(&others, |p| p.name.as_str(), filter);
        if shown.is_empty() {
            list = list.push(widgets::secondary("No other playlists of your own.").size(13));
        }
        for playlist in shown {
            let into = Msg::Into {
                id: playlist.id.clone(),
                name: playlist.name.clone(),
            };
            list = list.push(
                button(text(playlist.name.as_str()).size(13))
                    .width(Fill)
                    .style(theme::row)
                    .padding([4, 8])
                    .on_press(message(into)),
            );
        }
        let cancel = button(text("Cancel").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press(message(Msg::Cancel));
        iced::widget::container(column![list, cancel].spacing(8))
            .padding(8)
            .width(Fill)
            .style(theme::menu)
            .into()
    }
}
