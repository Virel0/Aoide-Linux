//! The steps of turning two-factor sign-in on, off and resetting it, driven
//! by the answers the server gives. Nothing is sent: the tasks are dropped.

use aoide_melete::{Device, MeleteClient, ServerAddress, TwoFactorSetup};

use super::*;

fn client() -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse("http://127.0.0.1:9").unwrap(),
        Device::new("Test", "device-1"),
    )
    .unwrap()
}

fn setup() -> TwoFactorSetup {
    TwoFactorSetup {
        secret: "JBSWY3DPEHPK3PXP".into(),
        uri: "otpauth://totp/Melete:alice?secret=JBSWY3DPEHPK3PXP".into(),
        qr: Some("<svg xmlns=\"http://www.w3.org/2000/svg\"/>".into()),
    }
}

fn run(state: &mut State, msg: Msg) {
    let client = client();
    let _ = state.update(
        msg,
        &Ctx {
            client: &client,
            key: 1,
        },
    );
}

fn typed(s: &str) -> Secret {
    Secret::new(s.to_owned())
}

#[test]
fn turning_on_goes_password_then_secret_then_on_and_drops_the_secret() {
    let mut state = State::default();
    run(&mut state, Msg::TurnOn);
    run(&mut state, Msg::Password(typed("fixture-pass")));
    run(&mut state, Msg::SendPassword);
    assert!(matches!(state.step, Step::Password { busy: true, .. }));
    run(&mut state, Msg::SetupStarted(Ok(setup())));
    let Step::Scan { pending, .. } = &state.step else {
        panic!("expected the secret to scan, got {:?}", state.step);
    };
    assert!(pending.qr.is_some());
    assert!(!format!("{:?}", state.step).contains("JBSWY3DP"));

    run(&mut state, Msg::Code("123456".into()));
    run(&mut state, Msg::Confirm);
    run(&mut state, Msg::Confirmed(Ok(())));
    assert!(matches!(state.step, Step::Idle));
    assert_eq!(state.note.as_deref(), Some(words::TURNED_ON));
}

#[test]
fn a_wrong_code_keeps_the_secret_and_clears_the_code() {
    let mut state = State::default();
    run(&mut state, Msg::SetupStarted(Ok(setup())));
    run(&mut state, Msg::Code("000000".into()));
    run(&mut state, Msg::Confirm);
    run(
        &mut state,
        Msg::Confirmed(Err((Some(Field::Code), "wrong".into()))),
    );
    let Step::Scan {
        code,
        problem,
        busy,
        ..
    } = &state.step
    else {
        panic!("expected the secret still, got {:?}", state.step);
    };
    assert_eq!(
        (code.as_str(), problem.as_deref(), *busy),
        ("", Some("wrong"), false)
    );
}

#[test]
fn turning_off_points_at_the_wrong_field_and_clears_only_it() {
    let mut state = State::default();
    run(&mut state, Msg::TurnOff);
    run(&mut state, Msg::Password(typed("fixture-pass")));
    run(&mut state, Msg::Code("654321".into()));
    run(&mut state, Msg::SendOff);
    run(
        &mut state,
        Msg::TurnedOff(Err((Some(Field::Password), "wrong".into()))),
    );
    let Step::Off {
        password,
        code,
        problem,
        ..
    } = &state.step
    else {
        panic!("expected the off step, got {:?}", state.step);
    };
    assert!(password.is_empty());
    assert_eq!(code, "654321");
    assert_eq!(problem.as_ref().map(|p| p.0), Some(Some(Field::Password)));

    run(&mut state, Msg::Password(typed("fixture-pass")));
    run(&mut state, Msg::SendOff);
    run(&mut state, Msg::TurnedOff(Ok(())));
    assert!(matches!(state.step, Step::Idle));
    assert_eq!(state.note.as_deref(), Some(words::TURNED_OFF));
}

#[test]
fn reset_turns_it_off_then_asks_for_a_new_secret_at_once() {
    let mut state = State::default();
    run(&mut state, Msg::Reset);
    run(&mut state, Msg::Password(typed("fixture-pass")));
    run(&mut state, Msg::Code("654321".into()));
    run(&mut state, Msg::SendOff);
    run(&mut state, Msg::TurnedOff(Ok(())));
    // Waiting for the new secret, with no password left on screen.
    let Step::Password { password, busy, .. } = &state.step else {
        panic!("expected a new setup, got {:?}", state.step);
    };
    assert!(password.is_empty() && *busy);
    run(&mut state, Msg::SetupStarted(Ok(setup())));
    assert!(matches!(state.step, Step::Scan { .. }));
}

#[test]
fn leaving_drops_the_secret_and_whatever_was_typed() {
    let mut state = State::default();
    run(&mut state, Msg::SetupStarted(Ok(setup())));
    state.forget();
    assert!(matches!(state.step, Step::Idle));

    let mut settings = crate::screens::settings::State {
        confirm_sign_out: false,
        hidden: crate::screens::Load::Loading,
        category: crate::screens::settings::Category::Account,
        account: crate::screens::settings::account::State::default(),
    };
    run(
        &mut settings.account.two_factor,
        Msg::SetupStarted(Ok(setup())),
    );
    let _ = settings.update(crate::screens::settings::Message::Category(
        crate::screens::settings::Category::General,
    ));
    assert!(matches!(settings.account.two_factor.step, Step::Idle));
}

#[test]
fn a_conflict_goes_back_to_the_status_with_the_reason() {
    let mut state = State::default();
    run(&mut state, Msg::SetupStarted(Ok(setup())));
    run(&mut state, Msg::Confirm);
    run(
        &mut state,
        Msg::Confirmed(Err((None, "no setup waiting".into()))),
    );
    assert!(matches!(state.step, Step::Idle));
    assert_eq!(state.note.as_deref(), Some("no setup waiting"));
}
