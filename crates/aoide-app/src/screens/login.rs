//! The sign-in screen: server address, username, password and Connect, or
//! Quick Connect: a code to approve in Melete's web UI instead of a password.

use iced::widget::{button, column, container, text, text_input};
use iced::{Center, Element, Fill, Length};

use crate::session::SignInForm;
use crate::theme;

pub const SERVER: &str = "login-server";
pub const USERNAME: &str = "login-username";
pub const PASSWORD: &str = "login-password";

#[derive(Debug, Default)]
pub struct State {
    pub form: SignInForm,
    pub error: Option<String>,
    pub busy: bool,
    /// Quick Connect's code while it waits to be approved.
    pub quick: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Server(String),
    Username(String),
    Password(String),
    /// Enter in a field: on to the next one.
    Next(&'static str),
    Submit,
    /// Quick Connect pressed.
    QuickConnect,
    /// A Quick Connect code to show, or why there is none.
    QuickStarted(Result<String, String>),
    /// Stop waiting for the code to be approved.
    CancelQuick,
}

/// What the app should do after a form message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Focus(&'static str),
    /// Sign in with this form. The password has already been cleared from the
    /// screen's own copy.
    SignIn(SignInForm),
    /// Start Quick Connect with this server.
    StartQuickConnect(String),
    /// Stop asking whether the code was approved.
    StopQuickConnect,
}

impl State {
    /// The form, offering the server this device last signed in to.
    pub fn new(last_server: Option<String>) -> Self {
        Self {
            form: SignInForm {
                server: last_server.unwrap_or_default(),
                ..SignInForm::default()
            },
            ..Self::default()
        }
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::Server(value) => {
                self.form.server = value;
                Action::None
            }
            Message::Username(value) => {
                self.form.username = value;
                Action::None
            }
            Message::Password(value) => {
                self.form.password = value;
                Action::None
            }
            Message::Next(id) => Action::Focus(id),
            Message::Submit => {
                if self.busy || !self.form.can_submit() {
                    return Action::None;
                }
                if let Err(error) = self.form.validate() {
                    self.error = Some(error);
                    return Action::None;
                }
                self.busy = true;
                self.error = None;
                let form = self.form.clone();
                self.form.password.clear();
                Action::SignIn(form)
            }
            Message::QuickConnect => {
                if self.busy || self.form.server.trim().is_empty() {
                    return Action::None;
                }
                self.busy = true;
                self.error = None;
                Action::StartQuickConnect(self.form.server.clone())
            }
            Message::QuickStarted(Ok(code)) => {
                self.quick = Some(code);
                Action::None
            }
            Message::QuickStarted(Err(error)) => {
                self.failed(error);
                Action::None
            }
            Message::CancelQuick => {
                self.quick = None;
                self.busy = false;
                Action::StopQuickConnect
            }
        }
    }

    /// Where typing starts: the username when the address is already there.
    pub fn first_field(&self) -> &'static str {
        if self.form.server.trim().is_empty() {
            SERVER
        } else {
            USERNAME
        }
    }

    /// The server answered no.
    pub fn failed(&mut self, error: String) {
        self.busy = false;
        self.quick = None;
        self.error = Some(error);
    }

    pub fn view(&self) -> Element<'_, Message> {
        let bold = crate::widgets::BOLD;
        let label = |s: &'static str| {
            text(s).size(13).style(|t| text::Style {
                color: Some(theme::secondary(t)),
            })
        };
        let editable = !self.busy;

        let server = text_input("melete.local:8100", &self.form.server)
            .id(SERVER)
            .on_input_maybe(editable.then_some(Message::Server))
            .on_submit(Message::Next(USERNAME))
            .padding(10)
            .style(theme::field);
        let username = text_input("", &self.form.username)
            .id(USERNAME)
            .on_input_maybe(editable.then_some(Message::Username))
            .on_submit(Message::Next(PASSWORD))
            .padding(10)
            .style(theme::field);
        let password = text_input("", &self.form.password)
            .id(PASSWORD)
            .secure(true)
            .on_input_maybe(editable.then_some(Message::Password))
            .on_submit(Message::Submit)
            .padding(10)
            .style(theme::field);

        let connect = button(
            text(if self.busy {
                "Connecting…"
            } else {
                "Connect"
            })
            .width(Fill)
            .align_x(Center),
        )
        .width(Fill)
        .padding(10)
        .style(theme::filled)
        .on_press_maybe((editable && self.form.can_submit()).then_some(Message::Submit));

        let mut form = column![
            text("Aoide").size(34).font(bold),
            text("Sign in to your Melete server.").style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
            column![
                label("Server"),
                server,
                text(
                    "Home addresses (melete.local, or a local network IP) connect over plain HTTP; \
                     anything else uses HTTPS. Type a full address to choose."
                )
                .size(12)
                .style(|t| text::Style {
                    color: Some(theme::secondary(t)),
                }),
            ]
            .spacing(6),
            column![label("Username"), username].spacing(6),
            column![label("Password"), password].spacing(6),
        ]
        .spacing(20)
        .width(Length::Fixed(380.0));

        if let Some(error) = &self.error {
            form = form.push(
                container(text(error.as_str()).size(14))
                    .padding(10)
                    .width(Fill)
                    .style(theme::error_box),
            );
        }
        form = form.push(connect);
        form = form.push(self.quick_view(editable));

        container(form).center(Fill).style(theme::page).into()
    }
}

impl State {
    /// Quick Connect: the button, or the code while it waits.
    fn quick_view(&self, editable: bool) -> Element<'_, Message> {
        let quiet = |t: &iced::Theme| text::Style {
            color: Some(theme::secondary(t)),
        };
        let Some(code) = &self.quick else {
            return button(
                text("Sign in with Quick Connect")
                    .width(Fill)
                    .align_x(Center),
            )
            .width(Fill)
            .padding(10)
            .style(theme::tonal)
            .on_press_maybe(
                (editable && !self.form.server.trim().is_empty()).then_some(Message::QuickConnect),
            )
            .into();
        };
        column![
            text(code.as_str())
                .size(34)
                .font(crate::widgets::BOLD)
                .width(Fill)
                .align_x(Center),
            text(
                "In Melete's web UI, signed in on another device, open Quick Connect and enter \
                 this code. It works for five minutes; this window signs in once it is approved."
            )
            .size(13)
            .style(quiet),
            button(text("Cancel").width(Fill).align_x(Center))
                .width(Fill)
                .padding(8)
                .style(theme::tonal)
                .on_press(Message::CancelQuick),
        ]
        .spacing(10)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> State {
        let mut state = State::new(Some("music.example.net".into()));
        state.update(Message::Username("alice".into()));
        state.update(Message::Password("pw".into()));
        state
    }

    #[test]
    fn the_last_server_is_offered() {
        assert_eq!(
            State::new(Some("a.example".into())).form.server,
            "a.example"
        );
    }

    #[test]
    fn submitting_hands_over_the_form_and_forgets_the_password() {
        let mut state = filled();
        let Action::SignIn(form) = state.update(Message::Submit) else {
            panic!("expected a sign-in");
        };
        assert_eq!(form.password, "pw");
        assert!(state.form.password.is_empty());
        assert!(state.busy);
        // A second Enter while connecting does nothing.
        assert_eq!(state.update(Message::Submit), Action::None);
    }

    #[test]
    fn a_bad_address_is_reported_without_sending_anything() {
        let mut state = filled();
        state.update(Message::Server("ftp://nope".into()));
        assert_eq!(state.update(Message::Submit), Action::None);
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("isn't a valid server address")
        );
        assert!(!state.busy);
    }

    #[test]
    fn a_failure_unlocks_the_form() {
        let mut state = filled();
        state.update(Message::Submit);
        state.failed("Your username or password wasn't accepted.".into());
        assert!(!state.busy);
        assert_eq!(
            state.error.as_deref(),
            Some("Your username or password wasn't accepted.")
        );
    }

    #[test]
    fn quick_connect_needs_an_address_and_runs_once() {
        let mut state = State::default();
        assert_eq!(state.update(Message::QuickConnect), Action::None);
        state.update(Message::Server("music.example.net".into()));
        assert_eq!(
            state.update(Message::QuickConnect),
            Action::StartQuickConnect("music.example.net".into())
        );
        assert_eq!(state.update(Message::QuickConnect), Action::None);
        state.update(Message::QuickStarted(Ok("482913".into())));
        assert_eq!(state.quick.as_deref(), Some("482913"));
        assert_eq!(state.update(Message::CancelQuick), Action::StopQuickConnect);
        assert!(state.quick.is_none() && !state.busy);
    }

    #[test]
    fn a_quick_connect_failure_unlocks_the_form() {
        let mut state = State::new(Some("music.example.net".into()));
        state.update(Message::QuickConnect);
        state.update(Message::QuickStarted(Ok("1".into())));
        state.failed("The code ran out before it was approved.".into());
        assert!(state.quick.is_none() && !state.busy);
        assert!(state.error.is_some());
    }

    #[test]
    fn typing_starts_after_a_remembered_address() {
        assert_eq!(State::new(None).first_field(), SERVER);
        assert_eq!(
            State::new(Some("music.example.net".into())).first_field(),
            USERNAME
        );
    }

    #[test]
    fn enter_moves_through_the_fields() {
        let mut state = State::default();
        assert_eq!(
            state.update(Message::Next(USERNAME)),
            Action::Focus(USERNAME)
        );
    }
}
