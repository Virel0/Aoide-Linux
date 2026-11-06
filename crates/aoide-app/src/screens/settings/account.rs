//! Settings → Account: the server and the user signed in, their profile
//! picture, their password, two-factor sign-in, and for an administrator the
//! server's users. Each part has its own state, messages and view below.
//!
//! Passwords, codes, a new two-factor secret and a password made for someone
//! are held only while the pane is open: leaving it drops them
//! ([`State::forget_secrets`]), and `Debug` never prints them, so a log line
//! can't carry one.

use std::fmt;
use std::future::Future;

use aoide_melete::{Error, Me, MeleteClient};
use iced::widget::{button, column, row, text};
use iced::{Center, Element, Fill, Task};

use super::{Message as Settings, section};
use crate::app::Message as App;
use crate::config::Account;
use crate::screens::PageMsg;
use crate::{theme, widgets};

pub mod admin;
pub mod password;
pub mod picture;
pub mod two_factor;

/// Something typed that must never be logged or kept: `Debug` prints none of it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(typed: String) -> Self {
        Self(typed)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Debug, Default)]
pub struct State {
    /// Who is signed in, as `/api/v1/me` says: `None` until it answers.
    pub me: Option<Result<Me, String>>,
    pub picture: picture::State,
    pub password: password::State,
    pub two_factor: two_factor::State,
    pub admin: admin::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The pane was opened: ask the server who is signed in.
    Open,
    Me(Result<Me, String>),
    Picture(picture::Msg),
    Password(password::Msg),
    TwoFactor(two_factor::Msg),
    Admin(admin::Msg),
}

/// What a part's update can reach.
pub struct Ctx<'a> {
    pub client: &'a MeleteClient,
    pub key: u64,
}

/// The app message that brings `msg` to the Account pane of page `key`.
pub fn wrap(key: u64, msg: Msg) -> App {
    App::Page(key, PageMsg::Settings(Settings::Account(msg)))
}

/// Runs `request` for the pane of page `key`. A rejected token signs out, as
/// on every screen; any other answer comes back as `done` makes it.
pub fn ask<T: Send + 'static>(
    key: u64,
    request: impl Future<Output = Result<T, Error>> + Send + 'static,
    done: impl Fn(Result<T, Error>) -> Msg + Send + 'static,
) -> Task<App> {
    Task::perform(request, move |result| match result {
        Err(Error::Unauthorized) => App::TokenRejected,
        result => wrap(key, done(result)),
    })
}

/// A failure no part words for itself, as every screen words it.
pub fn plainly(error: &Error) -> String {
    crate::failure::describe(error).summary
}

impl State {
    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Open => {
                let client = ctx.client.clone();
                let me = ask(ctx.key, async move { client.me().await }, |r| {
                    Msg::Me(r.map_err(|e| plainly(&e)))
                });
                Task::batch([me, self.two_factor.load(ctx)])
            }
            Msg::Me(result) => {
                let admin = result.as_ref().is_ok_and(|me| me.is_admin);
                self.me = Some(result);
                if admin {
                    self.admin.load(ctx)
                } else {
                    Task::none()
                }
            }
            Msg::Picture(msg) => self.picture.update(msg, ctx),
            Msg::Password(msg) => self.password.update(msg, ctx),
            Msg::TwoFactor(msg) => self.two_factor.update(msg, ctx),
            Msg::Admin(msg) => self.admin.update(msg, ctx),
        }
    }

    /// Drops every password, code and secret held: the pane was left.
    pub fn forget_secrets(&mut self) {
        self.password.forget();
        self.two_factor.forget();
        self.admin.forget();
        self.picture.close_gallery();
    }

    /// The signed-in user's id, once the server has said.
    fn my_id(&self) -> Option<i64> {
        self.me.as_ref()?.as_ref().ok().map(|me| me.id)
    }
}

/// The Account pane: who is signed in and Sign out first, then each part.
pub fn view<'a>(
    settings: &'a super::State,
    account: Option<&'a Account>,
    app: &'a crate::app::App,
    key: u64,
) -> Element<'a, App> {
    let state = &settings.account;
    let mut pane = column![signed_in_section(settings, account, app, key)].spacing(24);
    let name = account.map_or("", |a| a.username.as_str());
    let picture = app.session.as_ref().and_then(|s| s.avatar.as_deref());
    pane = pane
        .push(state.picture.view(name, picture, &app.artwork, key))
        .push(state.password.view(key))
        .push(state.two_factor.view(name, key));
    if let Some(Ok(me)) = &state.me
        && me.is_admin
    {
        pane = pane.push(state.admin.view(state.my_id(), &app.artwork, key));
    }
    pane.into()
}

/// The server, the user, and Sign out with its confirmation.
fn signed_in_section<'a>(
    state: &'a super::State,
    account: Option<&'a Account>,
    app: &'a crate::app::App,
    key: u64,
) -> Element<'a, App> {
    let message = move |m: Settings| App::Page(key, PageMsg::Settings(m));
    let mut account_rows = column![];
    if let Some(account) = account {
        let picture = app.session.as_ref().and_then(|s| s.avatar.as_deref());
        account_rows = account_rows
            .push(row![
                text("Server").width(Fill),
                widgets::secondary(account.server.as_str())
            ])
            .push(
                row![
                    text("Signed in as").width(Fill),
                    widgets::avatar(&account.username, picture, &app.artwork, 22.0),
                    widgets::secondary(account.username.as_str())
                ]
                .spacing(8)
                .align_y(Center),
            );
    }
    account_rows = account_rows.push(if state.confirm_sign_out {
        Element::from(
            column![
                text("Sign out of Aoide? You'll need your server address and password to sign back in."),
                row![
                    button("Sign out")
                        .style(theme::destructive)
                        .padding([6, 14])
                        .on_press(App::SignOut),
                    button("Cancel")
                        .style(theme::tonal)
                        .padding([6, 14])
                        .on_press(message(Settings::ConfirmSignOut(false))),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
    } else {
        button("Sign out")
            .style(theme::destructive)
            .padding([6, 14])
            .on_press(message(Settings::SignOut))
            .into()
    });
    section("", account_rows, "")
}

/// A small button as Settings draws them.
pub fn action<'a>(
    label: &'a str,
    style: fn(&iced::Theme, button::Status) -> button::Style,
    press: Option<App>,
) -> Element<'a, App> {
    button(text(label).size(13))
        .style(style)
        .padding([6, 14])
        .on_press_maybe(press)
        .into()
}

/// A labelled field for a password, its characters hidden.
pub fn secret_field<'a>(
    label: &'a str,
    value: &'a Secret,
    id: &'static str,
    on_input: Option<impl Fn(Secret) -> App + 'a>,
    on_submit: Option<App>,
) -> Element<'a, App> {
    let mut input = iced::widget::text_input("", value.expose())
        .id(id)
        .secure(true)
        .padding(8)
        .style(theme::field);
    if let Some(on_input) = on_input {
        input = input.on_input(move |typed| on_input(Secret::new(typed)));
    }
    if let Some(submit) = on_submit {
        input = input.on_submit(submit);
    }
    column![text(label).size(13), input].spacing(4).into()
}

/// A six-digit code field: anything but digits is dropped as it is typed.
pub fn code_field<'a>(
    label: &'a str,
    value: &'a str,
    id: &'static str,
    on_input: Option<impl Fn(String) -> App + 'a>,
    on_submit: Option<App>,
) -> Element<'a, App> {
    let mut input = iced::widget::text_input("123456", value)
        .id(id)
        .padding(8)
        .width(140)
        .style(theme::field);
    if let Some(on_input) = on_input {
        input = input.on_input(move |typed| on_input(digits(&typed)));
    }
    if let Some(submit) = on_submit {
        input = input.on_submit(submit);
    }
    column![text(label).size(13), input].spacing(4).into()
}

/// The first six digits of what was typed.
pub fn digits(typed: &str) -> String {
    typed.chars().filter(char::is_ascii_digit).take(6).collect()
}

/// A line saying what went wrong, under the field or button it is about.
pub fn problem<'a>(words: &'a str) -> Element<'a, App> {
    widgets::plain_failure(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints() {
        let typed = Secret::new("fixture-pass".into());
        assert_eq!(format!("{typed:?}"), "<redacted>");
        let msg = Msg::Password(password::Msg::Current(typed));
        assert!(!format!("{msg:?}").contains("fixture-pass"));
    }

    #[test]
    fn a_code_keeps_six_digits() {
        assert_eq!(digits("12 34-56"), "123456");
        assert_eq!(digits("1234567"), "123456");
        assert_eq!(digits("abc"), "");
    }
}
