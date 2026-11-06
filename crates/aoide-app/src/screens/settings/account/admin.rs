//! For an administrator: the server's users, each with Give a New Password.
//! The server makes the password and signs that user out everywhere; it is
//! shown once, to pass on privately, and dropped when the pane is left.

use aoide_melete::{Error, NewPassword, User};
use iced::widget::{column, row, text};
use iced::{Center, Element, Fill, Font, Task};

use super::{Ctx, Msg as Account, action, ask, plainly, problem, wrap};
use crate::app::Message as App;
use crate::artwork::Artwork;
use crate::screens::settings::section;
use crate::{theme, widgets};

#[derive(Debug, Default)]
pub struct State {
    pub users: Option<Result<Vec<User>, String>>,
    /// The user Give a New Password was pressed for, asking once more.
    pub confirming: Option<i64>,
    /// The user whose password is being made.
    pub busy: Option<i64>,
    /// The password made, until Done or the pane is left.
    pub given: Option<NewPassword>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Users(Result<Vec<User>, String>),
    Give(i64),
    Confirm(i64),
    Cancel,
    Given(Result<NewPassword, String>),
    Copy,
    Done,
}

fn send(key: u64) -> impl Fn(Msg) -> App + Copy {
    move |m| wrap(key, Account::Admin(m))
}

impl State {
    /// Asks for the server's users.
    pub fn load(&self, ctx: &Ctx<'_>) -> Task<App> {
        let client = ctx.client.clone();
        ask(ctx.key, async move { client.users().await }, |r| {
            Account::Admin(Msg::Users(r.map_err(|e| refused(&e))))
        })
    }

    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Users(users) => self.users = Some(users),
            Msg::Give(id) => {
                self.confirming = Some(id);
                self.given = None;
                self.problem = None;
            }
            Msg::Cancel => self.confirming = None,
            Msg::Confirm(id) => {
                self.confirming = None;
                self.busy = Some(id);
                let client = ctx.client.clone();
                return ask(
                    ctx.key,
                    async move { client.give_new_password(id).await },
                    |r| Account::Admin(Msg::Given(r.map_err(|e| refused(&e)))),
                );
            }
            Msg::Given(result) => {
                self.busy = None;
                match result {
                    Ok(given) => self.given = Some(given),
                    Err(words) => {
                        self.problem = Some(words);
                        // A user gone from the server goes from the list.
                        return self.load(ctx);
                    }
                }
            }
            Msg::Copy => {
                if let Some(given) = &self.given {
                    return iced::clipboard::write(given.password.clone());
                }
            }
            Msg::Done => self.given = None,
        }
        Task::none()
    }

    /// Drops a password made for someone, and any question asked.
    pub fn forget(&mut self) {
        self.given = None;
        self.confirming = None;
    }

    pub fn view<'a>(&'a self, me: Option<i64>, artwork: &'a Artwork, key: u64) -> Element<'a, App> {
        let mut rows = column![];
        if let Some(given) = &self.given {
            rows = rows.push(given_rows(given, key));
        }
        if let Some(words) = &self.problem {
            rows = rows.push(problem(words));
        }
        match &self.users {
            None => rows = rows.push(widgets::secondary("Loading…")),
            Some(Err(words)) => rows = rows.push(problem(words)),
            Some(Ok(users)) => {
                for user in users {
                    rows = rows.push(self.user_row(user, me == Some(user.id), artwork, key));
                }
            }
        }
        section(
            "Users",
            rows,
            "Give someone a new password when they can't sign in; they are signed out \
             everywhere. It leaves their two-factor sign-in on: for someone who has lost their \
             authenticator app, run melete user reset-password on the server instead. Change \
             your own password under Password.",
        )
    }

    fn user_row<'a>(
        &'a self,
        user: &'a User,
        is_me: bool,
        artwork: &'a Artwork,
        key: u64,
    ) -> Element<'a, App> {
        let send = send(key);
        let name = user.username.as_str();
        let mut label = column![text(name)];
        if user.is_admin || is_me {
            let tag = match (is_me, user.is_admin) {
                (true, _) => "You",
                _ => "Administrator",
            };
            label = label.push(widgets::secondary(tag).size(12));
        }
        let line = row![
            widgets::avatar(name, user.avatar_url.as_deref(), artwork, 28.0),
            label.width(Fill),
        ]
        .spacing(10)
        .align_y(Center);
        let line = if is_me {
            line
        } else if self.busy == Some(user.id) {
            line.push(widgets::secondary("Making a password…").size(13))
        } else {
            let idle = self.busy.is_none();
            line.push(action(
                "Give a new password…",
                theme::tonal,
                idle.then(|| send(Msg::Give(user.id))),
            ))
        };
        if self.confirming != Some(user.id) {
            return line.into();
        }
        column![
            line,
            text(format!(
                "Give {name} a new password? Their old one stops working, and they're signed \
                 out on every device and app, with their API keys. Two-factor sign-in stays as \
                 it is."
            )),
            row![
                action(
                    "Give a new password",
                    theme::destructive,
                    Some(send(Msg::Confirm(user.id)))
                ),
                action("Cancel", theme::tonal, Some(send(Msg::Cancel))),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into()
    }
}

/// The password made, once: copy it, pass it on privately, then Done.
fn given_rows(given: &NewPassword, key: u64) -> Element<'_, App> {
    let send = send(key);
    column![
        text(format!("{}'s new password:", given.username)),
        row![
            text(given.password.as_str())
                .font(Font::MONOSPACE)
                .width(Fill),
            action("Copy", theme::tonal, Some(send(Msg::Copy))),
            action("Done", theme::filled, Some(send(Msg::Done))),
        ]
        .spacing(8)
        .align_y(Center),
        widgets::secondary(
            "It's shown only this once. Pass it on to them privately, in person or in a \
             message only they can read, and ask them to change it after signing in."
        )
        .size(13),
    ]
    .spacing(6)
    .into()
}

/// The server's refusal in words.
pub fn refused(error: &Error) -> String {
    match error {
        Error::Http { status: 403, .. } => {
            "Only an administrator of this server can do this.".to_owned()
        }
        Error::Http { status: 409, .. } => {
            "That's your own account: change your password under Password instead.".to_owned()
        }
        Error::NotFound { .. } => "That user isn't on the server any more.".to_owned(),
        other => plainly(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_says_what_happened() {
        let http = |status| Error::Http {
            status,
            path: "api/v1/users/2/password".into(),
            detail: None,
            reason: None,
        };
        assert!(refused(&http(403)).contains("Only an administrator"));
        assert!(refused(&http(409)).contains("your own account"));
        let gone = Error::NotFound {
            path: "api/v1/users/9/password".into(),
        };
        assert!(refused(&gone).contains("any more"));
    }

    #[test]
    fn a_password_made_never_prints() {
        let given = NewPassword {
            username: "robin".into(),
            password: "fixture-made-pass".into(),
        };
        let msg = Msg::Given(Ok(given));
        assert!(!format!("{msg:?}").contains("fixture-made-pass"));
    }
}
