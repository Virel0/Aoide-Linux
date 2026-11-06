//! Two-factor sign-in in words: the server's refusals, pointed at the field
//! they are about, and its changes as a person would say them.

use aoide_melete::{ChangeKind, Error, MadeFrom, TwoFactorChange, WRONG_CODE, WRONG_PASSWORD};

use super::Refusal;
use crate::screens::settings::account::plainly;

/// Which field a refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Password,
    Code,
}

/// What was being done when the server refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Setup,
    Confirm,
    Off,
    Reset,
}

pub const TURNED_ON: &str = "Two-factor sign-in is on. Signing in with your password now also \
     asks for the code your authenticator app shows. Devices signed in already stay signed in.";

pub const TURNED_OFF: &str = "Two-factor sign-in is off. Signing in needs only your password now.";

/// What to do without the authenticator app, for `username`.
pub fn lost(username: &str) -> String {
    format!(
        "Lost the device with your authenticator app? Then it can't be turned off here: ask an \
         administrator of your server to run \u{201c}melete user reset-password {username}\u{201d} \
         on it and to remove the second factor when it asks. That gives you a new password too."
    )
}

pub const WRONG_SIGN_IN_CODES: &str = "A wrong code at sign-in means the password was right. \
     If that wasn't you, someone knows your password: change it.";

/// The server's refusal in words, and the field to fix.
pub fn refused(error: &Error, doing: Doing) -> Refusal {
    match error {
        Error::Http {
            status: 403,
            reason,
            ..
        } => match reason.as_deref() {
            Some(WRONG_PASSWORD) => (
                Some(Field::Password),
                "That password isn't right. Type it again.".to_owned(),
            ),
            Some(WRONG_CODE) => (
                Some(Field::Code),
                "That code isn't right. Type the code your authenticator app shows now; if it \
                 keeps failing, check that this computer's and the phone's clocks are right."
                    .to_owned(),
            ),
            _ => (None, plainly(error)),
        },
        Error::Http { status: 409, .. } => (None, conflict(doing).to_owned()),
        Error::Http { status: 429, .. } => (
            None,
            "Too many wrong passwords or codes. Two-factor sign-in can't be changed for 15 \
             minutes; try again then."
                .to_owned(),
        ),
        other => (None, plainly(other)),
    }
}

/// HTTP 409: the server's state isn't what the pane showed.
fn conflict(doing: Doing) -> &'static str {
    match doing {
        Doing::Setup => "Two-factor sign-in is on already. Turn it off first to set it up again.",
        Doing::Confirm => {
            "There's no setup waiting for a code: it was finished or started again elsewhere. \
             Start again if it's still off."
        }
        Doing::Off | Doing::Reset => "Two-factor sign-in is off already.",
    }
}

/// One change, as a line: what, where, and how long ago.
pub fn change(change: &TwoFactorChange, now_ms: i64) -> String {
    let what = match (change.change, change.made_from) {
        (ChangeKind::TwoFactorOn, MadeFrom::Server) => "Turned on on the server".to_owned(),
        (ChangeKind::TwoFactorOn, _) => "Turned on from your account".to_owned(),
        (ChangeKind::TwoFactorOff, MadeFrom::Server) => {
            "Turned off on the server by an administrator".to_owned()
        }
        (ChangeKind::TwoFactorOff, _) => "Turned off from your account".to_owned(),
        (ChangeKind::WrongSignInCode, _) if change.count == 1 => {
            "A wrong code typed at sign-in".to_owned()
        }
        (ChangeKind::WrongSignInCode, _) => {
            format!("{} wrong codes typed at sign-in", change.count)
        }
        (ChangeKind::Other, _) => "Another change".to_owned(),
    };
    let when = crate::screens::sync_status::ago(change.at.saturating_mul(1000), now_ms);
    format!("{what}, {when}")
}

/// Whether any of `changes` is a wrong code at sign-in.
pub fn wrong_codes_seen(changes: &[TwoFactorChange]) -> bool {
    changes
        .iter()
        .any(|c| c.change == ChangeKind::WrongSignInCode)
}

/// The secret in groups of four, easier to type.
pub fn grouped(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    chars
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(status: u16, reason: Option<&str>) -> Error {
        Error::Http {
            status,
            path: "api/v1/me/two-factor/off".into(),
            detail: Some("refused".into()),
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn a_wrong_password_or_code_points_at_its_field() {
        let wrong = refusal(403, Some(WRONG_PASSWORD));
        assert_eq!(refused(&wrong, Doing::Off).0, Some(Field::Password));
        let wrong = refusal(403, Some(WRONG_CODE));
        assert_eq!(refused(&wrong, Doing::Off).0, Some(Field::Code));
        assert_eq!(refused(&refusal(403, None), Doing::Off).0, None);
    }

    #[test]
    fn conflicts_and_lockouts_say_what_to_do() {
        let (field, words) = refused(&refusal(409, None), Doing::Setup);
        assert_eq!(field, None);
        assert!(words.contains("on already"), "{words}");
        let (_, words) = refused(&refusal(409, None), Doing::Off);
        assert!(words.contains("off already"), "{words}");
        let (_, words) = refused(&refusal(429, None), Doing::Confirm);
        assert!(words.contains("15 minutes"), "{words}");
    }

    #[test]
    fn changes_read_as_a_person_would_say_them() {
        let now = 1_700_000_000_000;
        let at = |change, made_from, count| TwoFactorChange {
            change,
            made_from,
            at: 1_700_000_000 - 3 * 3_600,
            count,
        };
        assert_eq!(
            change(&at(ChangeKind::TwoFactorOn, MadeFrom::Account, 1), now),
            "Turned on from your account, 3 hours ago"
        );
        assert_eq!(
            change(&at(ChangeKind::TwoFactorOff, MadeFrom::Server, 1), now),
            "Turned off on the server by an administrator, 3 hours ago"
        );
        assert_eq!(
            change(&at(ChangeKind::WrongSignInCode, MadeFrom::SignIn, 4), now),
            "4 wrong codes typed at sign-in, 3 hours ago"
        );
        assert!(wrong_codes_seen(&[at(
            ChangeKind::WrongSignInCode,
            MadeFrom::SignIn,
            1
        )]));
        assert!(!wrong_codes_seen(&[at(
            ChangeKind::TwoFactorOn,
            MadeFrom::Account,
            1
        )]));
    }

    #[test]
    fn without_the_app_an_administrator_is_named_with_the_command() {
        let words = lost("robin");
        assert!(
            words.contains("melete user reset-password robin"),
            "{words}"
        );
    }

    #[test]
    fn the_secret_is_grouped_by_four() {
        assert_eq!(grouped("JBSWY3DPEHPK"), "JBSW Y3DP EHPK");
        assert_eq!(grouped("ABCDEF"), "ABCD EF");
    }
}
