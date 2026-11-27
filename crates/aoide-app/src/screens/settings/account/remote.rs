//! Remote access, in the Account pane: this computer on Melete's own tunnel,
//! for when Melete's usual address doesn't answer. The work is the app's
//! (`app::remote`); this draws it.

use iced::widget::{column, row, text};
use iced::{Element, Fill};

use super::{action, problem};
use crate::app::Message as App;
use crate::app::remote::Msg;
use crate::remote::{State, Way};
use crate::screens::settings::section;
use crate::{theme, widgets};

const FOOTER: &str = "When Melete's usual address doesn't answer, as away from home, Aoide \
    reaches it through Melete's own WireGuard tunnel instead. Only Aoide's own connections to \
    Melete go through it: no VPN app is needed, and nothing else on this computer changes.";

/// The section, for a session with `state`; `set_up` when this account's
/// device is on the tunnel.
pub fn view(state: &State, set_up: bool) -> Element<'_, App> {
    let mut rows = column![].spacing(10);
    if set_up {
        rows = rows.push(row![
            text("Reaching Melete").width(Fill),
            widgets::secondary(way_words(state.way.as_ref()))
        ]);
        let label = if state.busy {
            "Removing…"
        } else {
            "Remove remote access"
        };
        let press = (!state.busy).then_some(App::Remote(Msg::Remove));
        rows = rows.push(action(label, theme::tonal, press));
    } else if state.offered == Some(false) {
        rows = rows.push(widgets::secondary(
            "Melete's own tunnel isn't set up on this server, so there's no remote access to set up.",
        ));
    } else {
        rows = rows.push(widgets::secondary(
            "Set this up while Aoide can reach Melete, as at home.",
        ));
        let label = if state.busy {
            "Setting up…"
        } else {
            "Set up remote access"
        };
        let press = (!state.busy).then_some(App::Remote(Msg::SetUp));
        rows = rows.push(action(label, theme::tonal, press));
    }
    if let Some(words) = &state.problem {
        rows = rows.push(problem(words));
    }
    section("Remote access", rows, FOOTER)
}

fn way_words(way: Option<&Way>) -> String {
    match way {
        None => "Checking…".to_owned(),
        Some(Way::Direct) => "At its usual address".to_owned(),
        Some(Way::Tunnel) => "Through Melete's tunnel".to_owned(),
        Some(Way::Neither(why)) => format!("Can't reach it: {why}"),
    }
}
