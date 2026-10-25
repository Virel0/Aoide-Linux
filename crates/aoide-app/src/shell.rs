//! The signed-in window: the sidebar, the content area with its back stack,
//! the Now Playing panel on the right, and the player bar along the bottom.

use iced::widget::{
    Column, button, center, column, container, mouse_area, opaque, row, rule, scrollable, space,
    stack, text,
};
use iced::{Center, Element, Fill, Length};

use crate::actions::{Menu, RowAction};
use crate::app::{App, Message, Signed};
use crate::icons::{Icon, Tone, icon};
use crate::screens::{Page, Section, View};
use crate::{now_playing, player_bar, screens, theme, widgets};

const SIDEBAR_WIDTH: f32 = 220.0;

pub fn view<'a>(app: &'a App, signed: &'a Signed) -> Element<'a, Message> {
    if let Some(full) = &app.listening.full {
        return crate::full_player::view(app, signed, full);
    }
    let main = row![sidebar(app, signed), content(app, signed)].height(Fill);
    let main = match signed.panel {
        Some(panel) => main
            .push(rule::vertical(1))
            .push(now_playing::view(app, signed, panel)),
        None => main,
    };
    let window = column![main, player_bar::view(app, signed)];
    let window: Element<'a, Message> = if let Some(popover) = crate::player_menu::overlay(app) {
        stack![window, popover].into()
    } else {
        match signed.menu.as_ref().filter(|m| m.collection.is_some()) {
            Some(menu) => stack![window, collection_menu(app, signed, menu)].into(),
            None => window.into(),
        }
    };
    // The release notes (and the other sheets of Settings → System) on top.
    let window: Element<'a, Message> = match crate::system::overlay(app) {
        Some(sheet) => stack![window, sheet].into(),
        None => window,
    };
    // Get Info over everything.
    match &signed.info {
        Some(sheet) => stack![window, crate::info::view(sheet, app.failure_details)].into(),
        None => window,
    }
}

/// An album's, artist's or playlist's actions, over everything: a tile in a
/// grid or a shelf has no row below it to open under. A click outside closes.
fn collection_menu<'a>(app: &'a App, signed: &'a Signed, menu: &'a Menu) -> Element<'a, Message> {
    let view = View {
        key: 0,
        artwork: &app.artwork,
        playing: None,
        menu: Some(menu),
        playlists: &signed.playlists,
        favourites: &signed.favourites,
        can_go_back: false,
        smart_search: false,
        details: app.failure_details,
    };
    let card = container(widgets::menu_strip(menu, &view)).max_width(560);
    opaque(
        mouse_area(center(opaque(card)).style(theme::scrim))
            .on_press(Message::Row(RowAction::Close)),
    )
}

fn sidebar<'a>(app: &'a App, signed: &'a Signed) -> Element<'a, Message> {
    let item = |label: String, glyph: Icon, section: Section| {
        let selected = signed.section == section
            || (section == Section::Playlists && signed.section == Section::JellyfinPlaylists);
        button(
            row![
                icon(
                    glyph,
                    18.0,
                    if selected {
                        Tone::Accent
                    } else {
                        Tone::Secondary
                    }
                ),
                crate::ellipsis::ellipsis(label).size(14),
            ]
            .spacing(10)
            .align_y(Center),
        )
        .width(Fill)
        .padding([6, 10])
        .style(if selected {
            theme::row_selected
        } else {
            theme::row
        })
        .on_press(Message::Section(section))
    };
    let glyph = |section: &Section| match section {
        Section::Home => Icon::Home,
        Section::Albums => Icon::Album,
        Section::Artists => Icon::Artist,
        Section::Songs => Icon::Song,
        Section::Genres => Icon::Genre,
        Section::Playlists | Section::Playlist(_) | Section::JellyfinPlaylists => Icon::Playlist,
        Section::Search => Icon::Search,
        Section::Settings => Icon::Settings,
        Section::Mixes => Icon::Mix,
        Section::Favourites => Icon::Heart,
        Section::Replay => Icon::Sparkle,
        Section::Collections | Section::Collection(_) => Icon::Genre,
    };

    let mut library = Column::new().spacing(2);
    for section in Section::LIBRARY {
        library = library.push(item(section.title().to_owned(), glyph(&section), section));
    }
    let mut lists = Column::new().spacing(2);
    for playlist in &signed.playlists {
        lists = lists.push(item(
            playlist.name.clone(),
            Icon::Playlist,
            Section::Playlist(playlist.id.clone()),
        ));
    }
    let heading = |s: &'static str| {
        container(
            text(s)
                .size(12)
                .font(widgets::SEMIBOLD)
                .style(|t| text::Style {
                    color: Some(theme::secondary(t)),
                }),
        )
        .padding([0, 10])
    };
    let mut body = column![
        container(text("Aoide").size(22).font(widgets::BOLD)).padding([4, 10]),
        library,
    ]
    .spacing(14);
    // Before the playlists, which can run long.
    body = body.push(heading("Collections")).push(
        screens::collections::sidebar(&app.config.collections, |label, section| {
            item(label, Icon::Genre, section).into()
        })
        .push(item(
            "New collection…".into(),
            Icon::Plus,
            Section::Collections,
        )),
    );
    if !signed.playlists.is_empty() {
        body = body.push(heading("Aoide playlists")).push(lists);
    }
    let mut footer = column![item("Settings".into(), Icon::Settings, Section::Settings)].spacing(4);
    if let Some(Err(status)) = &signed.sync_status {
        footer =
            footer.push(container(widgets::secondary(status.as_str()).size(11)).padding([0, 10]));
    }
    // The scrollbar has its own lane beside the list, so it never covers
    // the selected item's end.
    let list = scrollable(body)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .spacing(4),
        ))
        .height(Fill);
    container(column![list, footer].spacing(8).padding([16, 10]))
        .width(Length::Fixed(SIDEBAR_WIDTH))
        .height(Fill)
        .style(theme::side)
        .into()
}

fn content<'a>(app: &'a App, signed: &'a Signed) -> Element<'a, Message> {
    let mut body = Column::new().width(Fill).height(Fill);
    if signed.unknown_owner && signed.confirm_erase {
        body = body.push(banner(
            "Erase the listening history and playlists on this computer? This can't be undone.",
            Some((
                ("Cancel", Message::ConfirmErase(false)),
                ("Erase", Message::ResolveOwner { keep: false }),
            )),
        ));
    } else if signed.unknown_owner {
        body = body.push(banner(
            "This computer has listening history and playlists from before accounts were \
             recorded. Are they yours?",
            Some((
                ("Keep them", Message::ResolveOwner { keep: true }),
                ("Erase them", Message::ConfirmErase(true)),
            )),
        ));
    }
    if let Some(notice) = &app.notice {
        body = body.push(banner(notice, None));
    }
    if let Some(update) = crate::updates::banner(app) {
        body = body.push(update);
    }
    if let Some(reason) = app.playback.unavailable() {
        body = body.push(
            container(widgets::secondary(format!("No sound output: {reason}")).size(12))
                .padding([6, 28]),
        );
    }
    let Some(nav) = &signed.nav else {
        return container(body.push(widgets::loading()))
            .style(theme::page)
            .into();
    };
    let entry = nav.current();
    let view = View {
        key: entry.key,
        artwork: &app.artwork,
        playing: app.playback.current().map(|(_, q)| q.meta.id.as_str()),
        menu: signed.menu.as_ref(),
        playlists: &signed.playlists,
        favourites: &signed.favourites,
        can_go_back: nav.can_go_back(),
        smart_search: app.smart.has_key(),
        details: app.failure_details,
    };
    let page = match &entry.page {
        Page::Settings(state) => screens::settings::view(
            state,
            crate::sync::Status {
                available: signed.sync.is_some(),
                running: signed.throttle.is_running(),
                last: signed.sync_status.as_ref(),
            },
            &view,
            app,
            crate::listening::SettingsExtras {
                config: &app.config,
                listening: &app.listening,
                device_fell_back: app.playback.device_fell_back(),
            },
            content_width(app, signed) >= screens::settings::WIDE_ENOUGH,
        ),
        Page::Equalizer(state) => {
            screens::equalizer::view(state, &app.config.sound, app.playback.sample_rate(), &view)
        }
        page => page.view(&view),
    };
    container(body.push(page))
        .width(Fill)
        .height(Fill)
        .style(theme::page)
        .into()
}

/// The width the page has: the window's, less the sidebar and the panel.
fn content_width(app: &App, signed: &Signed) -> f32 {
    let panel = if signed.panel.is_some() {
        now_playing::WIDTH + 1.0
    } else {
        0.0
    };
    app.config.window.width - SIDEBAR_WIDTH - panel
}

/// A line across the top of the content: something to know, with a way to
/// answer or dismiss it.
type Choice<'a> = (&'a str, Message);

fn banner<'a>(message: &'a str, choices: Option<(Choice<'a>, Choice<'a>)>) -> Element<'a, Message> {
    let mut line = row![text(message).size(13).width(Fill)]
        .spacing(10)
        .align_y(Center);
    match choices {
        Some(((a, on_a), (b, on_b))) => {
            line = line
                .push(button(text(a).size(13)).style(theme::filled).on_press(on_a))
                .push(
                    button(text(b).size(13))
                        .style(theme::destructive)
                        .on_press(on_b),
                );
        }
        None => {
            line = line.push(
                button(icon(Icon::Close, 14.0, Tone::Secondary))
                    .style(theme::row)
                    .padding(4)
                    .on_press(Message::Dismiss),
            );
        }
    }
    container(container(line).padding(10).style(theme::menu).width(Fill))
        .padding([8, 28])
        .into()
}

/// An empty stretch, for pushing things apart in a row.
pub fn gap<'a>() -> Element<'a, Message> {
    space().width(Fill).into()
}
