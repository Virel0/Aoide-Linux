//! The two-factor section: the status and its changes, or the step under way.

use iced::widget::{Column, column, container, row, svg, text};
use iced::{Center, Element, Font};

use super::words::{self, Field};
use super::{Msg, Pending, Refusal, State, Step};
use crate::app::Message as App;
use crate::screens::settings::account::{
    Msg as Account, Secret, action, code_field, problem, secret_field, wrap,
};
use crate::screens::settings::section;
use crate::{theme, widgets};

const PASSWORD: &str = "account-two-factor-password";
const CODE: &str = "account-two-factor-code";

/// How many of the latest changes are listed.
const CHANGES_SHOWN: usize = 5;

/// The QR code's side, in logical pixels.
const QR_SIDE: f32 = 180.0;

fn send(key: u64) -> impl Fn(Msg) -> App + Copy {
    move |m| wrap(key, Account::TwoFactor(m))
}

impl State {
    pub fn view<'a>(&'a self, username: &str, key: u64) -> Element<'a, App> {
        let rows = match &self.step {
            Step::Idle => self.status_rows(key),
            Step::Password {
                password,
                busy,
                problem,
            } => password_rows(password, *busy, problem.as_deref(), key),
            Step::Scan {
                pending,
                code,
                busy,
                problem,
            } => scan_rows(pending, code, *busy, problem.as_deref(), key),
            Step::Off {
                password,
                code,
                reset,
                busy,
                problem,
            } => off_rows(password, code, *reset, *busy, problem.as_ref(), key),
        };
        let rows = rows.push(widgets::secondary(words::lost(username)).size(12));
        section(
            "Two-factor sign-in",
            rows,
            "With it on, signing in with your password also asks for a code from an \
             authenticator app on your phone, so your password alone isn't enough.",
        )
    }

    fn status_rows(&self, key: u64) -> Column<'_, App> {
        let send = send(key);
        let mut rows = column![];
        let status = match &self.status {
            None => return rows.push(widgets::secondary("Loading…")),
            Some(Err(words)) => return rows.push(problem(words)),
            Some(Ok(status)) => status,
        };
        let now = aoide_store::now_millis();
        let state = match (status.on, status.on_since) {
            (true, Some(since)) => format!(
                "On, since {}",
                crate::screens::sync_status::ago(since.saturating_mul(1000), now)
            ),
            (true, None) => "On".to_owned(),
            (false, _) => "Off".to_owned(),
        };
        let buttons = if status.on {
            row![
                action("Turn off…", theme::tonal, Some(send(Msg::TurnOff))),
                action("Reset…", theme::tonal, Some(send(Msg::Reset))),
            ]
        } else {
            row![action("Turn on…", theme::tonal, Some(send(Msg::TurnOn)))]
        };
        rows = rows.push(
            row![text(state).width(iced::Fill), buttons.spacing(8)]
                .spacing(8)
                .align_y(Center),
        );
        if let Some(note) = &self.note {
            rows = rows.push(widgets::secondary(note.as_str()).size(13));
        }
        if !status.changes.is_empty() {
            let mut list = column![text("Latest changes").size(13)].spacing(2);
            for change in status.changes.iter().take(CHANGES_SHOWN) {
                list = list.push(widgets::secondary(words::change(change, now)).size(13));
            }
            rows = rows.push(list);
        }
        if words::wrong_codes_seen(&status.changes) {
            rows = rows.push(problem(words::WRONG_SIGN_IN_CODES));
        }
        rows
    }
}

fn password_rows<'a>(
    password: &'a Secret,
    busy: bool,
    trouble: Option<&'a str>,
    key: u64,
) -> Column<'a, App> {
    let send = send(key);
    let submit = (!busy && !password.is_empty()).then(|| send(Msg::SendPassword));
    column![
        text(
            "Type your password to start. Then you'll get a code to scan with an authenticator app."
        ),
        secret_field(
            "Password",
            password,
            PASSWORD,
            (!busy).then_some(move |s| send(Msg::Password(s))),
            submit.clone(),
        ),
    ]
    .push(trouble.map(problem))
    .push(
        row![
            action(
                if busy { "Checking…" } else { "Continue" },
                theme::filled,
                submit
            ),
            action("Cancel", theme::tonal, (!busy).then(|| send(Msg::Cancel))),
        ]
        .spacing(8),
    )
}

fn scan_rows<'a>(
    pending: &'a Pending,
    code: &'a str,
    busy: bool,
    trouble: Option<&'a str>,
    key: u64,
) -> Column<'a, App> {
    let send = send(key);
    let submit = (!busy && code.len() == 6).then(|| send(Msg::Confirm));
    let mut rows = column![text(
        "Scan this with an authenticator app on your phone, or type the key into it. Then type \
         the six-digit code it shows. Two-factor sign-in isn't on until you do."
    )];
    if let Some(qr) = &pending.qr {
        rows = rows.push(
            container(svg(qr.clone()).width(QR_SIDE).height(QR_SIDE))
                .padding(8)
                .style(|_| container::Style::default().background(iced::Color::WHITE)),
        );
    }
    rows.push(
        row![
            column![
                text("Key").size(13),
                text(words::grouped(&pending.setup.secret)).font(Font::MONOSPACE),
            ]
            .spacing(2)
            .width(iced::Fill),
            action("Copy key", theme::tonal, Some(send(Msg::CopySecret))),
            action("Copy link", theme::tonal, Some(send(Msg::CopyLink))),
        ]
        .spacing(8)
        .align_y(Center),
    )
    .push(code_field(
        "Code from the app",
        code,
        CODE,
        (!busy).then_some(move |c| send(Msg::Code(c))),
        submit.clone(),
    ))
    .push(trouble.map(problem))
    .push(
        row![
            action(
                if busy { "Checking…" } else { "Turn on" },
                theme::filled,
                submit
            ),
            action("Cancel", theme::tonal, (!busy).then(|| send(Msg::Cancel))),
        ]
        .spacing(8),
    )
}

fn off_rows<'a>(
    password: &'a Secret,
    code: &'a str,
    reset: bool,
    busy: bool,
    trouble: Option<&'a Refusal>,
    key: u64,
) -> Column<'a, App> {
    let send = send(key);
    let ready = !busy && !password.is_empty() && code.len() == 6;
    let submit = ready.then(|| send(Msg::SendOff));
    let at = |field: Field| match trouble {
        Some((Some(f), words)) if *f == field => Some(problem(words)),
        _ => None,
    };
    let intro = if reset {
        "Reset turns two-factor sign-in off, then sets it up again with a new key, for a new \
         phone or app. Type your password and a code from the app you use now."
    } else {
        "Type your password and the code your authenticator app shows now."
    };
    let label = match (busy, reset) {
        (true, _) => "Checking…",
        (false, true) => "Reset",
        (false, false) => "Turn off",
    };
    column![
        text(intro),
        secret_field(
            "Password",
            password,
            PASSWORD,
            (!busy).then_some(move |s| send(Msg::Password(s))),
            None,
        ),
    ]
    .push(at(Field::Password))
    .push(code_field(
        "Code from the app",
        code,
        CODE,
        (!busy).then_some(move |c| send(Msg::Code(c))),
        submit.clone(),
    ))
    .push(at(Field::Code))
    .push(match trouble {
        Some((None, words)) => Some(problem(words)),
        _ => None,
    })
    .push(
        row![
            action(label, theme::destructive, submit),
            action("Cancel", theme::tonal, (!busy).then(|| send(Msg::Cancel))),
        ]
        .spacing(8),
    )
}
