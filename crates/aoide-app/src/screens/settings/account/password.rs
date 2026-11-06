//! Change password: the current one, the new one twice. The server signs out
//! every other session and API key of the account, and the pane says so
//! before and after.

use aoide_melete::{Error, PASSWORD_MAX_CHARS, PASSWORD_MIN_CHARS};
use iced::widget::column;
use iced::{Element, Task};

use super::{Ctx, Msg as Account, Secret, action, ask, plainly, problem, secret_field, wrap};
use crate::app::Message as App;
use crate::screens::settings::section;
use crate::theme;

const CURRENT: &str = "account-password-current";
const NEW: &str = "account-password-new";
const AGAIN: &str = "account-password-again";

/// Which field a problem is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Current,
    New,
    Again,
}

#[derive(Debug, Default)]
pub struct State {
    pub current: Secret,
    pub new: Secret,
    pub again: Secret,
    pub busy: bool,
    pub problem: Option<(Option<Field>, String)>,
    pub done: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Current(Secret),
    New(Secret),
    Again(Secret),
    Change,
    Changed(Result<(), (Option<Field>, String)>),
}

impl State {
    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Current(typed) => self.current = typed,
            Msg::New(typed) => self.new = typed,
            Msg::Again(typed) => self.again = typed,
            Msg::Change => return self.change(ctx),
            Msg::Changed(result) => {
                self.busy = false;
                match result {
                    Ok(()) => {
                        self.forget();
                        self.done = true;
                    }
                    Err((field, words)) => {
                        match field {
                            Some(Field::Current) => self.current = Secret::default(),
                            Some(Field::New | Field::Again) => {
                                self.new = Secret::default();
                                self.again = Secret::default();
                            }
                            None => {}
                        }
                        self.problem = Some((field, words));
                    }
                }
                return Task::none();
            }
        }
        self.problem = None;
        self.done = false;
        Task::none()
    }

    fn change(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        if let Some(problem) = check(&self.new, &self.again) {
            self.problem = Some(problem);
            return Task::none();
        }
        self.busy = true;
        self.problem = None;
        let client = ctx.client.clone();
        let (current, new) = (self.current.clone(), self.new.clone());
        ask(
            ctx.key,
            async move { client.change_password(current.expose(), new.expose()).await },
            |result| Account::Password(Msg::Changed(result.map_err(|e| refused(&e)))),
        )
    }

    /// Clears what was typed.
    pub fn forget(&mut self) {
        *self = Self::default();
    }

    pub fn view(&self, key: u64) -> Element<'_, App> {
        let send = move |m: Msg| wrap(key, Account::Password(m));
        let editable = !self.busy;
        let ready = !self.current.is_empty() && !self.new.is_empty() && !self.again.is_empty();
        let submit = (editable && ready).then(|| send(Msg::Change));
        let field_problem = |field: Field| match &self.problem {
            Some((Some(f), words)) if *f == field => Some(problem(words)),
            _ => None,
        };
        let mut rows = column![secret_field(
            "Current password",
            &self.current,
            CURRENT,
            editable.then_some(move |s| send(Msg::Current(s))),
            None,
        ),];
        rows = rows.push(field_problem(Field::Current));
        rows = rows.push(secret_field(
            "New password",
            &self.new,
            NEW,
            editable.then_some(move |s| send(Msg::New(s))),
            None,
        ));
        rows = rows.push(field_problem(Field::New));
        rows = rows.push(secret_field(
            "New password again",
            &self.again,
            AGAIN,
            editable.then_some(move |s| send(Msg::Again(s))),
            submit.clone(),
        ));
        rows = rows.push(field_problem(Field::Again));
        if let Some((None, words)) = &self.problem {
            rows = rows.push(problem(words));
        }
        if self.done {
            rows = rows.push(crate::widgets::secondary(CHANGED).size(13));
        }
        let label = if self.busy {
            "Changing…"
        } else {
            "Change password"
        };
        rows = rows.push(action(label, theme::tonal, submit));
        section("Password", rows, FOOTER)
    }
}

const FOOTER: &str = "Changing your password signs you out on every other device and app, and \
     stops every API key of yours from working. This computer stays signed in.";

const CHANGED: &str = "Your password is changed. Your other devices and apps are signed out, \
     and your API keys no longer work: sign in again there with the new password.";

/// What's wrong with the new password before anything is sent, if anything.
pub fn check(new: &Secret, again: &Secret) -> Option<(Option<Field>, String)> {
    let length = new.expose().chars().count();
    if !(PASSWORD_MIN_CHARS..=PASSWORD_MAX_CHARS).contains(&length) {
        return Some((Some(Field::New), length_words()));
    }
    (new != again).then(|| {
        (
            Some(Field::Again),
            "The two new passwords aren't the same. Type it again.".to_owned(),
        )
    })
}

fn length_words() -> String {
    format!("A new password needs {PASSWORD_MIN_CHARS} to {PASSWORD_MAX_CHARS} characters.")
}

/// The server's refusal in words, and the field it is about.
pub fn refused(error: &Error) -> (Option<Field>, String) {
    match error {
        Error::Http { status: 403, .. } => (
            Some(Field::Current),
            "That isn't your current password. Type it again.".to_owned(),
        ),
        Error::Http { status: 400, .. } => (Some(Field::New), length_words()),
        Error::Http { status: 429, .. } => (
            None,
            "Too many wrong passwords. Wait 15 minutes, then try again.".to_owned(),
        ),
        other => (None, plainly(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(s: &str) -> Secret {
        Secret::new(s.to_owned())
    }

    fn http(status: u16) -> Error {
        Error::Http {
            status,
            path: "api/v1/me/password".into(),
            detail: None,
            reason: None,
        }
    }

    #[test]
    fn the_new_password_is_checked_before_it_is_sent() {
        assert_eq!(
            check(&secret("short"), &secret("short")).map(|p| p.0),
            Some(Some(Field::New))
        );
        assert_eq!(
            check(&secret("long enough"), &secret("long enougH")).map(|p| p.0),
            Some(Some(Field::Again))
        );
        assert_eq!(check(&secret("long enough"), &secret("long enough")), None);
        let longest = "x".repeat(PASSWORD_MAX_CHARS + 1);
        assert!(check(&secret(&longest), &secret(&longest)).is_some());
    }

    #[test]
    fn each_refusal_points_at_its_field() {
        assert_eq!(refused(&http(403)).0, Some(Field::Current));
        assert_eq!(refused(&http(400)).0, Some(Field::New));
        let (field, words) = refused(&http(429));
        assert_eq!(field, None);
        assert!(words.contains("15 minutes"), "{words}");
    }
}
