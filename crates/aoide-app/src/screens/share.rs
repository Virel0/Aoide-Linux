//! Sharing one Aoide playlist with other people on the server, through the
//! sidecar's `/aoide/shares`: who it is shared with (or who shared it with
//! you), sharing it with someone else, stopping a share after asking, and the
//! edits held back because a share was revoked.
//!
//! Whom to share with is picked from the server's users when it lists them
//! for this account (administrators), and typed otherwise: an exact username
//! among those the sign-in screen shows, or a pasted user id.

use aoide_sidecar::{BlockedEdit, PlaylistShare, SidecarError};
use iced::widget::{Column, button, checkbox, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, Load, PageMsg, View};
use crate::app::{App, Message as AppMessage};
use crate::users::{self, ServerUser, Users};
use crate::{theme, widgets};

/// Everything the page shows, read together.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// This playlist's shares, both ways.
    pub shares: Vec<PlaylistShare>,
    pub users: Users,
    /// Edits to this playlist held back because its share was revoked.
    pub held: Vec<BlockedEdit>,
}

#[derive(Debug)]
pub struct State {
    pub id: String,
    name: String,
    /// The signed-in user's id: whose side of each share this is.
    me: String,
    snapshot: Load<Snapshot>,
    typed: String,
    can_edit: bool,
    /// The share about to be stopped, waiting for a yes.
    confirm: Option<String>,
    busy: bool,
    note: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Box<Snapshot>, String>),
    Typed(String),
    CanEdit(bool),
    /// Share with the user with this id.
    With(String),
    /// Share with whoever was typed.
    WithTyped,
    Stop(String),
    Confirm(Option<String>),
    Done(Result<String, String>),
}

/// The app's side: the sidecar calls.
#[derive(Debug, Clone)]
pub enum Msg {
    Load {
        key: u64,
        playlist: String,
    },
    Share {
        key: u64,
        playlist: String,
        grantee: String,
        can_edit: bool,
        name: String,
    },
    Revoke {
        key: u64,
        playlist: String,
        grantee: String,
        name: String,
    },
}

pub fn open(ctx: &Ctx<'_>, id: &str, name: &str) -> (State, Task<AppMessage>) {
    (
        State {
            id: id.to_owned(),
            name: name.to_owned(),
            me: ctx
                .client
                .credentials()
                .map(|c| c.user_id.clone())
                .unwrap_or_default(),
            snapshot: Load::Loading,
            typed: String::new(),
            can_edit: true,
            confirm: None,
            busy: false,
            note: None,
        },
        Task::done(AppMessage::Sharing(Msg::Load {
            key: ctx.key,
            playlist: id.to_owned(),
        })),
    )
}

/// Which share call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    List,
    Share,
    Revoke,
}

/// What a refused share call means, in the server's words where it gave any.
/// The client keeps no body for a 404 or a 403, so those are said here from
/// what the sidecar's contract means by them.
pub fn refusal(error: &SidecarError, call: Call) -> String {
    match (error, call) {
        (SidecarError::Absent { .. }, Call::List) => {
            "This server's Aoide sidecar doesn't share playlists yet.".to_owned()
        }
        (SidecarError::Absent { .. }, Call::Share) => {
            "This playlist hasn't reached the server yet. Sync, then share it.".to_owned()
        }
        (SidecarError::Absent { .. }, Call::Revoke) => "That share was already gone.".to_owned(),
        (SidecarError::Forbidden, Call::Share) => {
            "Only the playlist's owner can share it.".to_owned()
        }
        (SidecarError::Forbidden, _) => {
            "Only the playlist's owner, or the person it was shared with, can end a share."
                .to_owned()
        }
        (error, _) => error
            .server_text()
            .unwrap_or_else(|| crate::failure::sidecar(error).to_string()),
    }
}

/// The people listed by name, for the rows: who it's shared with and how.
pub fn describe_share(share: &PlaylistShare, me: &str, known: &[ServerUser]) -> String {
    let can = if share.can_edit {
        "can edit"
    } else {
        "can listen"
    };
    match &share.owner_user_id {
        Some(owner) if !users::same_id(owner, me) => {
            format!(
                "Shared with you by {} · you {can}",
                users::name_of(owner, known)
            )
        }
        _ => format!("{} · {can}", users::name_of(&share.grantee_user_id, known)),
    }
}

impl App {
    pub(crate) fn sharing(&mut self, message: Msg) -> Task<AppMessage> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let me = signed.connection.account.user_id.clone();
        let (Some(sidecar), Some(engine)) = (signed.sidecar.clone(), signed.sync.clone()) else {
            if let Msg::Load { key, .. } = message {
                return Task::done(AppMessage::Page(
                    key,
                    PageMsg::Share(Message::Loaded(Err(
                        "Sharing needs the Aoide sidecar on your server.".to_owned(),
                    ))),
                ));
            }
            return Task::none();
        };
        let client = signed.connection.client.clone();
        match message {
            Msg::Load { key, playlist } => Task::perform(
                async move {
                    // Through the engine, so a share given back lets its held
                    // edits go at once.
                    let shares = engine
                        .refresh_shares()
                        .await
                        .map_err(|e| refusal(&e, Call::List))?;
                    let users = users::users(client, me).await;
                    let held = engine
                        .blocked_edits()
                        .into_iter()
                        .filter(|edit| edit.playlist_id.as_deref() == Some(playlist.as_str()))
                        .collect();
                    Ok(Box::new(Snapshot {
                        shares: shares
                            .into_iter()
                            .filter(|s| s.playlist_id == playlist)
                            .collect(),
                        users,
                        held,
                    }))
                },
                move |result| AppMessage::Page(key, PageMsg::Share(Message::Loaded(result))),
            ),
            Msg::Share {
                key,
                playlist,
                grantee,
                can_edit,
                name,
            } => Task::perform(
                async move {
                    sidecar
                        .share_playlist(&playlist, &grantee, can_edit)
                        .await
                        .map(|()| format!("Shared with {name}."))
                        .map_err(|e| refusal(&e, Call::Share))
                },
                move |result| AppMessage::Page(key, PageMsg::Share(Message::Done(result))),
            ),
            Msg::Revoke {
                key,
                playlist,
                grantee,
                name,
            } => Task::perform(
                async move {
                    sidecar
                        .revoke_share(&playlist, &grantee)
                        .await
                        .map(|()| format!("No longer shared with {name}."))
                        .map_err(|e| refusal(&e, Call::Revoke))
                },
                move |result| AppMessage::Page(key, PageMsg::Share(Message::Done(result))),
            ),
        }
    }
}

impl State {
    fn known(&self) -> &[ServerUser] {
        match self.snapshot.ready().map(|s| &s.users) {
            Some(Users::Listed(list) | Users::Typed(list)) => list,
            None => &[],
        }
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<AppMessage> {
        let key = ctx.key;
        let reload = || {
            Task::done(AppMessage::Sharing(Msg::Load {
                key,
                playlist: self.id.clone(),
            }))
        };
        match message {
            Message::Loaded(result) => {
                self.busy = false;
                self.snapshot = Load::from(result.map(|s| *s));
                Task::none()
            }
            Message::Typed(typed) => {
                self.typed = typed;
                Task::none()
            }
            Message::CanEdit(on) => {
                self.can_edit = on;
                Task::none()
            }
            Message::With(grantee) => {
                let name = users::name_of(&grantee, self.known()).to_owned();
                self.busy = true;
                self.note = None;
                Task::done(AppMessage::Sharing(Msg::Share {
                    key,
                    playlist: self.id.clone(),
                    grantee,
                    can_edit: self.can_edit,
                    name,
                }))
            }
            Message::WithTyped => match users::resolve_typed(&self.typed, self.known()) {
                Some(grantee) => {
                    self.typed.clear();
                    self.update(Message::With(grantee), ctx)
                }
                None => {
                    self.note = Some(Err(format!(
                        "No one called \u{201c}{}\u{201d} is shown on this server's sign-in \
                         screen. Type their username exactly, or paste their user ID (an \
                         administrator can find it under Dashboard → Users).",
                        self.typed.trim()
                    )));
                    Task::none()
                }
            },
            Message::Stop(grantee) => {
                let name = users::name_of(&grantee, self.known()).to_owned();
                self.confirm = None;
                self.busy = true;
                self.note = None;
                Task::done(AppMessage::Sharing(Msg::Revoke {
                    key,
                    playlist: self.id.clone(),
                    grantee,
                    name,
                }))
            }
            Message::Confirm(grantee) => {
                self.confirm = grantee;
                Task::none()
            }
            Message::Done(result) => {
                self.busy = false;
                self.note = Some(result);
                reload()
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        let me = self.me.as_str();
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Share(m));
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
        body = body.push(section("Shared with", shared.into()));

        let already: Vec<&str> = snapshot
            .shares
            .iter()
            .map(|s| s.grantee_user_id.as_str())
            .collect();
        let edit = checkbox(self.can_edit)
            .label("They can edit it")
            .on_toggle(move |on| message(Message::CanEdit(on)));
        let add: Element<'a, AppMessage> = match &snapshot.users {
            Users::Listed(list) => {
                let mut people = Column::new().spacing(6).push(edit);
                let candidates: Vec<&ServerUser> = list
                    .iter()
                    .filter(|u| !already.iter().any(|a| users::same_id(a, &u.id)))
                    .collect();
                if candidates.is_empty() {
                    people = people.push(
                        widgets::secondary("It's shared with everyone else on this server.")
                            .size(13),
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
                                    (!self.busy).then(|| message(Message::With(user.id.clone()))),
                                ),
                        ]
                        .align_y(Center),
                    );
                }
                people.into()
            }
            Users::Typed(_) => {
                let ready = !self.typed.trim().is_empty() && !self.busy;
                column![
                    widgets::secondary(
                        "This server doesn't list its users for your account, so type their \
                         Jellyfin username, or paste their user ID."
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
        };
        body = body.push(section("Share with", add));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn share(grantee: &str, owner: Option<&str>, can_edit: bool) -> PlaylistShare {
        PlaylistShare {
            playlist_id: "p1".into(),
            grantee_user_id: grantee.into(),
            can_edit,
            owner_user_id: owner.map(str::to_owned),
        }
    }

    #[test]
    fn a_share_reads_from_this_users_side() {
        let known = [
            ServerUser {
                id: "u2".into(),
                name: "robin".into(),
            },
            ServerUser {
                id: "u3".into(),
                name: "sam".into(),
            },
        ];
        assert_eq!(
            describe_share(&share("u2", Some("me"), true), "me", &known),
            "robin · can edit"
        );
        assert_eq!(
            describe_share(&share("me", Some("u3"), false), "me", &known),
            "Shared with you by sam · you can listen"
        );
        assert_eq!(
            describe_share(&share("u9", None, true), "me", &known),
            "u9 · can edit"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refusal_is_the_servers_own_words_where_it_gave_any() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/aoide/shares"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "title": "Unknown user",
                "detail": "granteeUserId must be an existing Jellyfin user on this server.",
                "status": 400
            })))
            .mount(&server)
            .await;
        let sidecar = aoide_sidecar::SidecarClient::new(&server.uri(), "tok").unwrap();
        let error = sidecar.share_playlist("p1", "u2", true).await.unwrap_err();
        assert_eq!(
            refusal(&error, Call::Share),
            "Unknown user: granteeUserId must be an existing Jellyfin user on this server."
        );
        let absent = SidecarError::Absent {
            path: "aoide/shares".into(),
        };
        assert!(refusal(&absent, Call::Share).contains("hasn't reached the server"));
        assert!(refusal(&absent, Call::List).contains("doesn't share"));
        assert!(refusal(&SidecarError::Forbidden, Call::Share).contains("owner"));
    }
}
