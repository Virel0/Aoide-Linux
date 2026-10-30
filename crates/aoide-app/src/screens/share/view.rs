//! The share page as drawn: who it is shared with, whom to share it with,
//! and the edits held back.

use iced::widget::{Column, button, checkbox, column, container, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Message, Snapshot, State, describe_share};
use crate::app::Message as AppMessage;
use crate::screens::{Load, PageMsg, View};
use crate::users::{self, ServerUser, Users};
use crate::{theme, widgets};

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let title = format!("Share \u{201c}{}\u{201d}", self.name);
        let mut body = column![
            widgets::title_bar("", view.can_go_back),
            text(title).size(28).font(widgets::BOLD),
            widgets::secondary(
                "People you share with see this playlist in their Aoide apps. Those who can \
                 edit change it for everyone; their changes travel with the normal sync."
            )
            .size(13),
        ]
        .spacing(16);
        let snapshot = match &self.snapshot {
            Load::Loading => return widgets::page(body.push(widgets::loading())),
            Load::Failed(error) => return widgets::page(body.push(view.failure(error))),
            Load::Ready(snapshot) => snapshot,
        };
        body = body.push(section("Shared with", self.shared_with(snapshot, view.key)));
        body = body.push(section("Share with", self.people(snapshot, view.key)));
        if let Some(note) = &self.note {
            body = body.push(match note {
                Ok(note) => widgets::secondary(note.as_str()).size(13).into(),
                Err(error) => widgets::plain_failure(error),
            });
        }
        if !snapshot.held.is_empty() {
            let count = match snapshot.held.len() {
                1 => "1 of your edits to it is".to_owned(),
                n => format!("{n} of your edits to it are"),
            };
            body = body.push(section(
                "Held back",
                widgets::secondary(format!(
                    "{count} held: its owner no longer shares it with you for editing. They're \
                     kept, and go with the next sync if it's shared with you again."
                ))
                .size(13)
                .into(),
            ));
        }
        widgets::page(body.max_width(720))
    }

    /// Each share, with Stop Sharing (or Leave) and its question.
    fn shared_with<'a>(&'a self, snapshot: &'a Snapshot, key: u64) -> Element<'a, AppMessage> {
        let me = self.me.as_str();
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Share(m));
        let known = self.known();
        let mut shared = Column::new().spacing(8);
        if snapshot.shares.is_empty() {
            shared = shared.push(widgets::secondary("Not shared with anyone yet.").size(13));
        }
        for share in &snapshot.shares {
            let mine = share
                .owner_user_id
                .as_deref()
                .is_none_or(|owner| users::same_id(owner, me));
            // The owner stops a share; the person it was shared with can leave it.
            let (stop, question) = if mine {
                (
                    "Stop sharing",
                    "Stop sharing with them? Changes stop reaching them; nothing they already changed is undone.",
                )
            } else {
                (
                    "Leave",
                    "Leave this shared playlist? Its changes stop reaching you.",
                )
            };
            let grantee = share.grantee_user_id.clone();
            let line = row![text(describe_share(share, me, known)).size(14).width(Fill),]
                .spacing(8)
                .align_y(Center);
            let line = if self.confirm.as_deref() == Some(grantee.as_str()) {
                line.push(text(question).size(13))
                    .push(
                        button(text(stop).size(13))
                            .style(theme::destructive)
                            .padding([6, 12])
                            .on_press_maybe(
                                (!self.busy).then(|| message(Message::Stop(grantee.clone()))),
                            ),
                    )
                    .push(
                        button(text("Cancel").size(13))
                            .style(theme::tonal)
                            .padding([6, 12])
                            .on_press(message(Message::Confirm(None))),
                    )
            } else {
                line.push(
                    button(text(stop).size(13))
                        .style(theme::tonal)
                        .padding([6, 12])
                        .on_press(message(Message::Confirm(Some(grantee)))),
                )
            };
            shared = shared.push(line);
        }
        shared.into()
    }

    /// Whom to share with: the server's other users, each with Share, or a
    /// field to type one in where the server doesn't list them.
    fn people<'a>(&'a self, snapshot: &'a Snapshot, key: u64) -> Element<'a, AppMessage> {
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Share(m));
        let edit = checkbox(self.can_edit)
            .label("They can edit it")
            .on_toggle(move |on| message(Message::CanEdit(on)));
        match &snapshot.users {
            Users::Listed(list) => self.listed_users(snapshot, list, edit, key),
            Users::Typed(_) => {
                let ready = !self.typed.trim().is_empty() && !self.busy;
                column![
                    widgets::secondary(
                        "The server's users couldn't be listed, so type their username, or \
                         their user ID."
                    )
                    .size(13),
                    row![
                        text_input("Username", &self.typed)
                            .on_input(move |s| message(Message::Typed(s)))
                            .on_submit_maybe(ready.then(|| message(Message::WithTyped)))
                            .padding(8)
                            .style(theme::field),
                        button(text("Share"))
                            .style(theme::filled)
                            .padding([8, 14])
                            .on_press_maybe(ready.then(|| message(Message::WithTyped))),
                    ]
                    .spacing(10)
                    .align_y(Center),
                    edit,
                ]
                .spacing(8)
                .into()
            }
        }
    }

    /// The listed users it isn't shared with yet, each with Share.
    fn listed_users<'a>(
        &'a self,
        snapshot: &'a Snapshot,
        list: &'a [ServerUser],
        edit: impl Into<Element<'a, AppMessage>>,
        key: u64,
    ) -> Element<'a, AppMessage> {
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Share(m));
        let already: Vec<&str> = snapshot
            .shares
            .iter()
            .map(|s| s.grantee_user_id.as_str())
            .collect();
        let mut people = Column::new().spacing(6).push(edit);
        let candidates: Vec<&ServerUser> = list
            .iter()
            .filter(|u| !already.iter().any(|a| users::same_id(a, &u.id)))
            .collect();
        if candidates.is_empty() {
            people = people.push(
                widgets::secondary("It's shared with everyone else on this server.").size(13),
            );
        }
        for user in candidates {
            people = people.push(
                row![
                    text(user.name.as_str()).size(14).width(Fill),
                    button(text("Share").size(13))
                        .style(theme::filled)
                        .padding([6, 14])
                        .on_press_maybe(
                            (!self.busy).then(|| message(Message::With(user.id.clone())))
                        ),
                ]
                .align_y(Center),
            );
        }
        people.into()
    }
}

fn section<'a>(title: &'a str, content: Element<'a, AppMessage>) -> Element<'a, AppMessage> {
    column![
        text(title)
            .size(13)
            .font(widgets::SEMIBOLD)
            .style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
        container(content)
            .padding(14)
            .width(Fill)
            .style(theme::menu),
    ]
    .spacing(8)
    .into()
}
