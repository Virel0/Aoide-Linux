//! Two-factor sign-in: whether it is on and its latest changes; turning it on
//! (the password, then a secret to put in an authenticator app and its first
//! code); turning it off (the password and a current code); and Reset, which
//! is off and then on again with a new secret.
//!
//! The secret and its QR code live only in [`Step::Scan`], dropped once the
//! first code is taken, on Cancel, and when the pane is left.

use std::fmt;

use aoide_melete::{TwoFactorSetup, TwoFactorStatus};
use iced::Task;
use iced::widget::svg;

use super::{Ctx, Msg as Account, Secret, ask, plainly};
use crate::app::Message as App;

#[cfg(test)]
mod tests;
mod view;
pub mod words;

pub use words::Field;

/// A refusal in words, and the field it is about when there is one.
pub type Refusal = (Option<Field>, String);

#[derive(Debug, Default)]
pub struct State {
    /// What the server says of it: `None` until it answers.
    pub status: Option<Result<TwoFactorStatus, String>>,
    pub step: Step,
    /// What the last change came to.
    pub note: Option<String>,
}

#[derive(Debug, Default)]
pub enum Step {
    #[default]
    Idle,
    /// Turning it on: the password first.
    Password {
        password: Secret,
        busy: bool,
        problem: Option<String>,
    },
    /// The new secret, waiting for the first code from the app.
    Scan {
        pending: Pending,
        code: String,
        busy: bool,
        problem: Option<String>,
    },
    /// Turning it off; with `reset`, then on again.
    Off {
        password: Secret,
        code: String,
        reset: bool,
        busy: bool,
        problem: Option<Refusal>,
    },
}

/// A secret not yet confirmed, and its QR code drawn once. `Debug` prints
/// none of it.
pub struct Pending {
    pub setup: TwoFactorSetup,
    pub qr: Option<svg::Handle>,
}

impl fmt::Debug for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pending(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Status(Result<TwoFactorStatus, String>),
    TurnOn,
    TurnOff,
    Reset,
    Cancel,
    Password(Secret),
    Code(String),
    /// The password typed to turn it on: ask for a secret.
    SendPassword,
    SetupStarted(Result<TwoFactorSetup, Refusal>),
    Confirm,
    Confirmed(Result<(), Refusal>),
    SendOff,
    TurnedOff(Result<(), Refusal>),
    CopySecret,
    CopyLink,
}

fn wrap(msg: Msg) -> Account {
    Account::TwoFactor(msg)
}

impl State {
    /// Asks the server whether it is on.
    pub fn load(&self, ctx: &Ctx<'_>) -> Task<App> {
        let client = ctx.client.clone();
        ask(ctx.key, async move { client.two_factor().await }, |r| {
            wrap(Msg::Status(r.map_err(|e| plainly(&e))))
        })
    }

    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Status(status) => self.status = Some(status),
            Msg::TurnOn => self.begin(Step::password()),
            Msg::TurnOff => self.begin(Step::off(false)),
            Msg::Reset => self.begin(Step::off(true)),
            Msg::Cancel => self.step = Step::Idle,
            Msg::Password(typed) => self.step.set_password(typed),
            Msg::Code(typed) => self.step.set_code(typed),
            Msg::SendPassword => return self.send_password(ctx),
            Msg::SetupStarted(result) => return self.setup_started(result, ctx),
            Msg::Confirm => return self.confirm(ctx),
            Msg::Confirmed(result) => return self.confirmed(result, ctx),
            Msg::SendOff => return self.send_off(ctx),
            Msg::TurnedOff(result) => return self.turned_off(result, ctx),
            Msg::CopySecret | Msg::CopyLink => return self.copy(&msg),
        }
        Task::none()
    }

    /// Drops the secret, the password and the code.
    pub fn forget(&mut self) {
        self.step = Step::Idle;
    }

    fn begin(&mut self, step: Step) {
        self.note = None;
        self.step = step;
    }

    fn send_password(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Password {
            password,
            busy,
            problem,
        } = &mut self.step
        else {
            return Task::none();
        };
        *busy = true;
        *problem = None;
        setup(password.clone(), ctx)
    }

    fn setup_started(
        &mut self,
        result: Result<TwoFactorSetup, Refusal>,
        ctx: &Ctx<'_>,
    ) -> Task<App> {
        match result {
            Ok(setup) => {
                let qr = setup
                    .qr
                    .as_ref()
                    .map(|svg| svg::Handle::from_memory(svg.clone().into_bytes()));
                self.step = Step::Scan {
                    pending: Pending { setup, qr },
                    code: String::new(),
                    busy: false,
                    problem: None,
                };
                Task::none()
            }
            Err((_, words)) if matches!(self.step, Step::Password { .. }) => {
                self.step = Step::Password {
                    password: Secret::default(),
                    busy: false,
                    problem: Some(words),
                };
                Task::none()
            }
            Err((_, words)) => self.finish(words, ctx),
        }
    }

    fn confirm(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Scan {
            code,
            busy,
            problem,
            ..
        } = &mut self.step
        else {
            return Task::none();
        };
        *busy = true;
        *problem = None;
        let client = ctx.client.clone();
        let code = code.clone();
        ask(
            ctx.key,
            async move { client.confirm_two_factor(&code).await },
            |r| {
                wrap(Msg::Confirmed(
                    r.map_err(|e| words::refused(&e, words::Doing::Confirm)),
                ))
            },
        )
    }

    fn confirmed(&mut self, result: Result<(), Refusal>, ctx: &Ctx<'_>) -> Task<App> {
        match result {
            Ok(()) => self.finish(words::TURNED_ON.to_owned(), ctx),
            Err((Some(Field::Code), words)) => {
                if let Step::Scan {
                    code,
                    busy,
                    problem,
                    ..
                } = &mut self.step
                {
                    code.clear();
                    *busy = false;
                    *problem = Some(words);
                }
                Task::none()
            }
            Err((_, words)) => self.finish(words, ctx),
        }
    }

    fn send_off(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Off {
            password,
            code,
            reset,
            busy,
            problem,
        } = &mut self.step
        else {
            return Task::none();
        };
        *busy = true;
        *problem = None;
        let client = ctx.client.clone();
        let (password, code) = (password.clone(), code.clone());
        let doing = if *reset {
            words::Doing::Reset
        } else {
            words::Doing::Off
        };
        ask(
            ctx.key,
            async move { client.turn_off_two_factor(password.expose(), &code).await },
            move |r| wrap(Msg::TurnedOff(r.map_err(|e| words::refused(&e, doing)))),
        )
    }

    fn turned_off(&mut self, result: Result<(), Refusal>, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Off {
            password,
            code,
            reset,
            busy,
            problem,
        } = &mut self.step
        else {
            return Task::none();
        };
        match result {
            // Reset: on again at once, with the password just typed.
            Ok(()) if *reset => {
                let password = std::mem::take(password);
                self.step = Step::Password {
                    password: Secret::default(),
                    busy: true,
                    problem: None,
                };
                Task::batch([setup(password, ctx), self.load(ctx)])
            }
            Ok(()) => self.finish(words::TURNED_OFF.to_owned(), ctx),
            Err((Some(field), words)) => {
                match field {
                    Field::Password => *password = Secret::default(),
                    Field::Code => code.clear(),
                }
                *busy = false;
                *problem = Some((Some(field), words));
                Task::none()
            }
            Err((None, words)) => self.finish(words, ctx),
        }
    }

    /// Back to the status, saying what came of it, and the status asked again.
    fn finish(&mut self, note: String, ctx: &Ctx<'_>) -> Task<App> {
        self.step = Step::Idle;
        self.note = Some(note);
        self.load(ctx)
    }

    fn copy(&self, msg: &Msg) -> Task<App> {
        let Step::Scan { pending, .. } = &self.step else {
            return Task::none();
        };
        let copied = match msg {
            Msg::CopySecret => pending.setup.secret.clone(),
            _ => pending.setup.uri.clone(),
        };
        iced::clipboard::write(copied)
    }
}

/// Asks for a new secret with `password`.
fn setup(password: Secret, ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    ask(
        ctx.key,
        async move { client.start_two_factor(password.expose()).await },
        |r| {
            wrap(Msg::SetupStarted(
                r.map_err(|e| words::refused(&e, words::Doing::Setup)),
            ))
        },
    )
}

impl Step {
    fn password() -> Self {
        Step::Password {
            password: Secret::default(),
            busy: false,
            problem: None,
        }
    }

    fn off(reset: bool) -> Self {
        Step::Off {
            password: Secret::default(),
            code: String::new(),
            reset,
            busy: false,
            problem: None,
        }
    }

    fn set_password(&mut self, typed: Secret) {
        match self {
            Step::Password { password, .. } | Step::Off { password, .. } => *password = typed,
            _ => {}
        }
    }

    fn set_code(&mut self, typed: String) {
        match self {
            Step::Scan { code, .. } | Step::Off { code, .. } => *code = typed,
            _ => {}
        }
    }
}
