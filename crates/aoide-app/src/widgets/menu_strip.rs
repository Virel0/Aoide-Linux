//! The action strip under a row: its buttons, and the playlists to add to.

use iced::widget::{Column, Row, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill};

use super::page::plain_failure;
use super::{SEMIBOLD, secondary};
use crate::actions::{self, Menu, RowAction};
use crate::app::Message;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::screens::View;
use crate::theme;

/// The action strip under a row: Play Next, Play Last, Add to Playlist, the
/// taste flags, Favourite, Go to.
pub fn menu_strip<'a>(menu: &'a Menu, view: &View<'a>) -> Element<'a, Message> {
    let mut body = column![].spacing(10);
    if let Some((_, title)) = &menu.collection {
        body = body.push(text(title.as_str()).size(16).font(SEMIBOLD));
    }
    body = body.push(strip_actions(menu, view));
    if menu.loading {
        body = body.push(secondary("Fetching the songs…").size(13));
    }
    if let Some(error) = &menu.error {
        body = body.push(plain_failure(error));
    }
    if menu.adding {
        body = body
            .push(playlist_choices(menu, view))
            .push(new_playlist(menu));
    }
    container(body)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
}

/// The strip's buttons, wrapping when narrow.
fn strip_actions<'a>(menu: &'a Menu, view: &View<'a>) -> Element<'a, Message> {
    // A collection's actions wait for its songs.
    let ready = !menu.loading && !menu.tracks.is_empty();
    let enabled = ready || menu.collection.is_none();
    let action = |label: &'a str, act: RowAction| strip_button(label, act, enabled);
    let mut actions: Vec<Element<'a, Message>> = Vec::new();
    if menu.collection.is_some() {
        actions.push(action("Play", RowAction::Play));
        actions.push(action("Shuffle", RowAction::Shuffle));
    }
    actions.extend([
        action("Play next", RowAction::PlayNext),
        action("Play last", RowAction::PlayLast),
        action("Add to playlist…", RowAction::ShowPlaylists),
    ]);
    if menu.single().is_some() || menu.collection.is_some() {
        actions.push(action("Start radio", RowAction::StartRadio));
    }
    if let Some(track) = menu.single() {
        actions.extend(song_actions(menu, track, view, enabled));
    }
    actions.push(
        button(icon(Icon::Close, 14.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(Message::Row(RowAction::Close))
            .into(),
    );
    row(actions).spacing(6).wrap().vertical_spacing(6).into()
}

/// One song's own actions: the taste flags, Favourite, Go to, Get Info.
fn song_actions<'a>(
    menu: &'a Menu,
    track: &'a TrackMeta,
    view: &View<'a>,
    enabled: bool,
) -> Vec<Element<'a, Message>> {
    let action = |label: &'a str, act: RowAction| strip_button(label, act, enabled);
    let flags = menu.flags.unwrap_or_default();
    let favourite = view
        .favourites
        .get(&track.id)
        .copied()
        .unwrap_or(track.favourite);
    let mut actions = vec![
        check_button(
            "Not interested",
            flags.not_interested,
            RowAction::NotInterested,
        ),
        check_button("Don't count plays", flags.dont_count, RowAction::DontCount),
        action(
            if favourite {
                "Unfavourite"
            } else {
                "Favourite"
            },
            RowAction::Favourite,
        ),
    ];
    if track.album_id.is_some() {
        actions.push(action("Go to album", RowAction::GoToAlbum));
    }
    if track.artist_id.is_some() {
        actions.push(action("Go to artist", RowAction::GoToArtist));
    }
    actions.push(action("Get info", RowAction::GetInfo));
    actions
}

/// One of the strip's buttons; off while a collection's songs are awaited.
fn strip_button(label: &str, action: RowAction, enabled: bool) -> Element<'_, Message> {
    button(text(label).size(13))
        .style(theme::tonal)
        .padding([6, 10])
        .on_press_maybe(enabled.then_some(Message::Row(action)))
        .into()
}

/// A taste flag's button, with a check while it is set.
fn check_button(label: &'static str, on: bool, action: RowAction) -> Element<'static, Message> {
    let mut body = row![].spacing(4).align_y(Center);
    if on {
        body = body.push(icon(Icon::Check, 13.0, Tone::Accent));
    }
    button(body.push(text(label).size(13)))
        .style(theme::tonal)
        .padding([6, 10])
        .on_press(Message::Row(action))
        .into()
}

/// Add to Playlist's list of Aoide playlists, filtered by what is typed once
/// there are enough to need it.
fn playlist_choices<'a>(menu: &'a Menu, view: &View<'a>) -> Column<'a, Message> {
    let mut lists = column![].spacing(2);
    if view.playlists.len() > actions::PLAYLIST_SEARCH_AFTER {
        lists = lists.push(
            text_input("Find a playlist", &menu.filter)
                .on_input(|s| Message::Row(RowAction::Filter(s)))
                .padding(6)
                .size(13)
                .style(theme::field),
        );
    }
    let heading = |label: &'static str| secondary(label).size(12).font(SEMIBOLD);
    lists = lists.push(heading("Aoide playlists"));
    for playlist in actions::matching(view.playlists, |p| p.name.as_str(), &menu.filter) {
        lists = lists.push(
            button(text(playlist.name.as_str()).size(13))
                .width(Fill)
                .style(theme::row)
                .padding([4, 8])
                .on_press(Message::Row(RowAction::AddTo(playlist.id.clone()))),
        );
    }
    lists
}

/// A new playlist's name, and Create.
fn new_playlist(menu: &Menu) -> Row<'_, Message> {
    row![
        text_input("New playlist…", &menu.new_name)
            .on_input(|s| Message::Row(RowAction::NewName(s)))
            .on_submit(Message::Row(RowAction::CreatePlaylist))
            .padding(6)
            .size(13)
            .style(theme::field),
        button(text("Create").size(13))
            .style(theme::filled)
            .padding([6, 10])
            .on_press_maybe(
                (!menu.new_name.trim().is_empty())
                    .then_some(Message::Row(RowAction::CreatePlaylist))
            ),
    ]
    .spacing(6)
    .align_y(Center)
}
