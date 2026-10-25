//! The sign-in screen: server address, username, password, Connect, and
//! Find Servers on This Network (discover.rs) to pick an address instead of
//! typing one.

use aoide_jellyfin::DiscoveredServer;
use iced::widget::{button, column, container, row, text, text_input};
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
    /// The servers found on the network: `None` before looking, `Err` when
    /// the search itself failed.
    pub found: Option<Result<Vec<DiscoveredServer>, String>>,
    pub finding: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Server(String),
    Username(String),
    Password(String),
    /// Enter in a field: on to the next one.
    Next(&'static str),
    Submit,
    /// Find Servers on This Network.
    Find,
    Found(Result<Vec<DiscoveredServer>, String>),
    /// A found server's address, picked.
    Pick(String),
}

/// What the app should do after a form message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Focus(&'static str),
    /// Sign in with this form. The password has already been cleared from the
    /// screen's own copy.
    SignIn(SignInForm),
    /// Look for servers on the network.
    FindServers,
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
            Message::Find => {
                if self.finding || self.busy {
                    return Action::None;
                }
                self.finding = true;
                Action::FindServers
            }
            Message::Found(result) => {
                self.finding = false;
                self.found = Some(result);
                Action::None
            }
            Message::Pick(address) => {
                self.form.server = address;
                self.found = None;
                Action::Focus(USERNAME)
            }
        }
    }

    /// The server answered no.
    pub fn failed(&mut self, error: String) {
        self.busy = false;
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

        let server = text_input("jellyfin.local:8096", &self.form.server)
            .id(SERVER)
            .on_input_maybe(editable.then_some(Message::Server))
            .on_submit(Message::Next(USERNAME))
            .padding(10)
            .style(theme::field);
        let username = text_input("Username", &self.form.username)
            .id(USERNAME)
            .on_input_maybe(editable.then_some(Message::Username))
            .on_submit(Message::Next(PASSWORD))
            .padding(10)
            .style(theme::field);
        let password = text_input("Password", &self.form.password)
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
            text("Sign in to your Jellyfin server.").style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
            column![
                label("Server"),
                server,
                self.found_view(editable),
                text(
                    "Home addresses (jellyfin.local, or a local network IP) connect over plain HTTP; \
                     anything else uses HTTPS. Type a full address to choose."
                )
                .size(12)
                .style(|t| text::Style {
                    color: Some(theme::secondary(t)),
                }),
            ]
            .spacing(6),
            column![label("Account"), username, password].spacing(6),
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

        container(form).center(Fill).style(theme::page).into()
    }
}

impl State {
    /// Find Servers on This Network, and what it found.
    fn found_view(&self, editable: bool) -> Element<'_, Message> {
        let quiet = |t: &iced::Theme| text::Style {
            color: Some(theme::secondary(t)),
        };
        let mut part = column![
            button(
                text(if self.finding {
                    "Looking…"
                } else {
                    "Find Servers on This Network"
                })
                .size(13),
            )
            .style(theme::tonal)
            .padding([4, 10])
            .on_press_maybe((editable && !self.finding).then_some(Message::Find)),
        ]
        .spacing(4);
        match &self.found {
            None => {}
            Some(Err(error)) => {
                part = part.push(
                    text(format!("The network couldn't be searched: {error}"))
                        .size(12)
                        .style(quiet),
                );
            }
            Some(Ok(servers)) if servers.is_empty() => {
                part = part.push(
                    text(
                        "No server answered. Its discovery may be off, or a firewall in the way; \
                         type the address instead.",
                    )
                    .size(12)
                    .style(quiet),
                );
            }
            Some(Ok(servers)) => {
                for server in servers {
                    part = part.push(
                        button(
                            row![
                                text(crate::discover::label(server)).size(14).width(Fill),
                                // A server with no name is already listed by address.
                                text(if server.name.trim().is_empty() {
                                    ""
                                } else {
                                    server.address.as_str()
                                })
                                .size(12)
                                .style(quiet),
                            ]
                            .spacing(8)
                            .align_y(Center),
                        )
                        .width(Fill)
                        .padding([6, 10])
                        .style(theme::row)
                        .on_press_maybe(editable.then(|| Message::Pick(server.address.clone()))),
                    );
                }
            }
        }
        part.into()
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
    fn a_found_server_fills_in_the_address() {
        let mut state = State::default();
        assert_eq!(state.update(Message::Find), Action::FindServers);
        // Once at a time.
        assert_eq!(state.update(Message::Find), Action::None);
        let server = DiscoveredServer {
            id: "s1".into(),
            name: "Living Room".into(),
            address: "http://192.0.2.10:8096".into(),
            endpoint_address: None,
        };
        state.update(Message::Found(Ok(vec![server])));
        assert!(!state.finding);
        assert_eq!(
            state.update(Message::Pick("http://192.0.2.10:8096".into())),
            Action::Focus(USERNAME)
        );
        assert_eq!(state.form.server, "http://192.0.2.10:8096");
        assert!(state.found.is_none());
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
