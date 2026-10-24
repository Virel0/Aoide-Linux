//! The pieces every screen draws with: artwork, tiles, shelves, track rows
//! and the action strip under a row.

use aoide_store::PlaySource;
use iced::widget::{
    Column, button, column, container, grid, image, mouse_area, row, scrollable, space, text,
    text_input,
};
use iced::{Center, ContentFit, Element, Fill, Font, Length, Shrink, font};

use crate::actions::{self, Collection, Menu, PlayRequest, RowAction};
use crate::app::Message;
use crate::artwork::{ArtKey, Artwork};
use crate::icons::{Icon, Tone, icon};
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::screens::{Route, View};
use crate::theme;

pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

/// `3:05`, or `1:02:03` past an hour.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn secondary<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).style(|t| text::Style {
        color: Some(theme::secondary(t)),
    })
}

/// A cover at `size`, or a quiet square where there is none yet.
pub fn art<'a>(
    key: Option<&ArtKey>,
    artwork: &'a Artwork,
    size: impl Into<Length> + Copy,
    radius: f32,
) -> Element<'a, Message> {
    match key.and_then(|key| artwork.get(key)) {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(radius)
            .into(),
        None => container(space())
            .width(size)
            .height(size)
            .style(theme::placeholder)
            .into(),
    }
}

/// What a tile's `…` (or a right click on it) opens: the album's or
/// artist's actions.
fn tile_menu(tile: &Tile, route: &Route) -> Option<Message> {
    let collection = match route {
        Route::Album { id, .. } => Collection::Album(id.clone()),
        Route::Artist { id, .. } => Collection::Artist(id.clone()),
        Route::Playlist { id } => Collection::Playlist(id.clone()),
        Route::Genre { .. } => return None,
    };
    Some(Message::Row(RowAction::OpenCollection {
        anchor: format!("tile:{}", tile.id),
        collection,
        title: tile.title.clone(),
    }))
}

/// A tile's title and subtitle, with its `…` beside them.
fn tile_caption<'a>(tile: &'a Tile, menu: Option<Message>) -> Element<'a, Message> {
    let words = column![
        text(tile.title.as_str())
            .size(14)
            .font(SEMIBOLD)
            .wrapping(text::Wrapping::None),
        secondary(tile.subtitle.as_str())
            .size(13)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(2)
    .width(Fill)
    .clip(true);
    match menu {
        Some(menu) => row![
            words,
            button(icon(Icon::More, 16.0, Tone::Secondary))
                .style(theme::row)
                .padding(4)
                .on_press(menu),
        ]
        .align_y(Center)
        .into(),
        None => words.into(),
    }
}

/// A tile that opens what it shows; a right click opens its actions.
fn tile_button<'a>(
    body: impl Into<Element<'a, Message>>,
    route: Route,
    menu: Option<Message>,
) -> Element<'a, Message> {
    let button = button(body)
        .padding(4)
        .style(theme::row)
        .on_press(Message::Open(route));
    match menu {
        Some(menu) => mouse_area(button).on_right_press(menu).into(),
        None => button.into(),
    }
}

/// An album or artist tile that opens it.
fn tile_body<'a>(
    tile: &'a Tile,
    artwork: &'a Artwork,
    art_size: Length,
    menu: Option<Message>,
) -> Column<'a, Message> {
    column![
        art(tile.art.as_ref(), artwork, art_size, 8.0),
        tile_caption(tile, menu),
    ]
    .spacing(4)
    .clip(true)
}

/// A grid of tiles that fills the width, a few columns or many.
pub fn tile_grid<'a>(
    tiles: &'a [Tile],
    artwork: &'a Artwork,
    open: impl Fn(&Tile) -> Route,
) -> Element<'a, Message> {
    let cells = tiles.iter().map(|tile| {
        let route = open(tile);
        let menu = tile_menu(tile, &route);
        let body = column![
            container(art(tile.art.as_ref(), artwork, Fill, 8.0)).height(Fill),
            tile_caption(tile, menu.clone()),
        ]
        .spacing(4)
        .clip(true);
        tile_button(body, route, menu)
    });
    grid(cells)
        .fluid(210)
        .spacing(16)
        .height(grid::aspect_ratio(100, 128))
        .into()
}

/// A shelf: one row of tiles, scrolled sideways.
pub fn shelf<'a>(
    title: &'a str,
    tiles: &'a [Tile],
    artwork: &'a Artwork,
    open: impl Fn(&Tile) -> Route,
) -> Element<'a, Message> {
    let row = row(tiles.iter().map(|tile| {
        let route = open(tile);
        let menu = tile_menu(tile, &route);
        tile_button(
            tile_body(tile, artwork, Length::Fixed(160.0), menu.clone()).width(160),
            route,
            menu,
        )
    }))
    .spacing(12)
    .padding(iced::Padding::ZERO.bottom(14));
    column![
        text(title).size(20).font(BOLD),
        scrollable(row).horizontal().width(Fill),
    ]
    .spacing(10)
    .into()
}

/// A screen's title, with a back button when there is somewhere to go back to.
pub fn title_bar<'a>(title: &'a str, back: bool) -> Element<'a, Message> {
    let mut bar = row![].spacing(8).align_y(Center);
    if back {
        bar = bar.push(
            button(icon(Icon::Back, 20.0, Tone::Accent))
                .style(theme::row)
                .padding(4)
                .on_press(Message::Back),
        );
    }
    bar.push(text(title).size(28).font(BOLD)).into()
}

/// A page's whole body: scrolled, padded.
pub fn page<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(container(content).padding([24, 28]).width(Fill))
        .height(Fill)
        .into()
}

pub fn loading<'a>() -> Element<'a, Message> {
    container(secondary("Loading…")).padding(20).into()
}

pub fn failure<'a>(error: &'a str) -> Element<'a, Message> {
    container(text(error))
        .padding(12)
        .style(theme::error_box)
        .into()
}

pub fn empty<'a>(message: &'a str) -> Element<'a, Message> {
    container(secondary(message)).padding(20).into()
}

/// How a track list looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListStyle {
    /// Track numbers instead of covers: an album's own list.
    pub numbered: bool,
    pub show_album: bool,
}

/// A list of tracks. A click plays the list from that track; the `…` opens
/// the action strip under the row.
pub fn track_list<'a>(
    tracks: &'a [TrackMeta],
    view: &View<'a>,
    style: ListStyle,
    source: PlaySource,
) -> Element<'a, Message> {
    let mut list = Column::with_capacity(tracks.len()).spacing(2);
    for (index, track) in tracks.iter().enumerate() {
        let anchor = format!("{}:{index}", view.key);
        list = list.push(track_row(track, &anchor, view, style, move || {
            Message::Play(PlayRequest::from(tracks, index, source))
        }));
        if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
            list = list.push(menu_strip(menu, view));
        }
    }
    list.into()
}

/// One track row.
pub fn track_row<'a>(
    track: &'a TrackMeta,
    anchor: &str,
    view: &View<'a>,
    style: ListStyle,
    play: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    let playing = view.playing == Some(track.id.as_str());
    let lead: Element<'a, Message> = if style.numbered {
        let label = if playing {
            "♪".to_owned()
        } else {
            track
                .track_number
                .map(|n| n.to_string())
                .unwrap_or_default()
        };
        container(secondary(label).size(13))
            .width(28)
            .align_x(Center)
            .into()
    } else {
        art(
            track
                .artwork
                .as_ref()
                .map(|(i, t)| ArtKey::new(i, t))
                .as_ref(),
            view.artwork,
            Length::Fixed(40.0),
            4.0,
        )
    };
    let subtitle = if style.show_album && !track.album.is_empty() {
        format!("{} · {}", track.artist, track.album)
    } else {
        track.artist.clone()
    };
    let favourite = view
        .favourites
        .get(&track.id)
        .copied()
        .unwrap_or(track.favourite);
    let anchor = anchor.to_owned();
    let more = button(icon(Icon::More, 18.0, Tone::Secondary))
        .style(theme::row)
        .padding(6)
        .on_press_with(move || {
            Message::Row(RowAction::Open {
                anchor: anchor.clone(),
                tracks: vec![track.clone()],
            })
        });
    let body = row![
        lead,
        column![
            text(track.title.as_str())
                .size(14)
                .wrapping(text::Wrapping::None)
                .style(move |t| text::Style {
                    color: playing.then(|| theme::accent(t)),
                }),
            secondary(subtitle).size(12).wrapping(text::Wrapping::None),
        ]
        .spacing(2)
        .width(Fill)
        .clip(true),
    ]
    .spacing(12)
    .align_y(Center);
    let body = if favourite {
        body.push(icon(Icon::HeartFilled, 14.0, Tone::Accent))
    } else {
        body
    };
    let body = body
        .push(
            secondary(
                track
                    .duration_ms
                    .map(|ms| duration(ms.max(0) as u64))
                    .unwrap_or_default(),
            )
            .size(13),
        )
        .push(more);
    button(body)
        .padding([4, 8])
        .width(Fill)
        .style(if playing {
            theme::row_selected
        } else {
            theme::row
        })
        .on_press_with(play)
        .into()
}

/// The action strip under a row: Play Next, Play Last, Add to Playlist, the
/// taste flags, Favourite, Go to.
pub fn menu_strip<'a>(menu: &'a Menu, view: &View<'a>) -> Element<'a, Message> {
    let action = |label: &'a str, action: RowAction| {
        button(text(label).size(13))
            .style(theme::tonal)
            .padding([6, 10])
            .on_press(Message::Row(action))
    };
    let check = |label: &'static str, on: bool, act: RowAction| {
        let text_label = if on {
            format!("✓ {label}")
        } else {
            label.to_owned()
        };
        button(text(text_label).size(13))
            .style(theme::tonal)
            .padding([6, 10])
            .on_press(Message::Row(act))
    };
    // A collection's actions wait for its songs.
    let ready = !menu.loading && !menu.tracks.is_empty();
    let plain = action;
    let action = move |label: &'a str, act: RowAction| {
        let button = plain(label, act);
        if ready || menu.collection.is_none() {
            button
        } else {
            button.on_press_maybe(None)
        }
    };
    let mut actions: Vec<Element<'a, Message>> = Vec::new();
    if menu.collection.is_some() {
        actions.push(action("Play", RowAction::Play).into());
        actions.push(action("Shuffle", RowAction::Shuffle).into());
    }
    actions.extend([
        action("Play Next", RowAction::PlayNext).into(),
        action("Play Last", RowAction::PlayLast).into(),
        action("Add to Playlist…", RowAction::ShowPlaylists).into(),
    ]);
    if let Some(track) = menu.single() {
        let flags = menu.flags.unwrap_or_default();
        let favourite = view
            .favourites
            .get(&track.id)
            .copied()
            .unwrap_or(track.favourite);
        actions.push(
            check(
                "Not Interested",
                flags.not_interested,
                RowAction::NotInterested,
            )
            .into(),
        );
        actions.push(check("Don't Count Plays", flags.dont_count, RowAction::DontCount).into());
        actions.push(
            action(
                if favourite {
                    "Unfavourite"
                } else {
                    "Favourite"
                },
                RowAction::Favourite,
            )
            .into(),
        );
        if track.album_id.is_some() {
            actions.push(action("Go to Album", RowAction::GoToAlbum).into());
        }
        if track.artist_id.is_some() {
            actions.push(action("Go to Artist", RowAction::GoToArtist).into());
        }
    }
    actions.push(
        button(icon(Icon::Close, 14.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(Message::Row(RowAction::Close))
            .into(),
    );
    let actions = row(actions).spacing(6).wrap().vertical_spacing(6);

    let mut body = column![].spacing(10);
    if let Some((_, title)) = &menu.collection {
        body = body.push(text(title.as_str()).size(16).font(SEMIBOLD));
    }
    body = body.push(actions);
    if menu.loading {
        body = body.push(secondary("Fetching the songs…").size(13));
    }
    if let Some(error) = &menu.error {
        body = body.push(failure(error));
    }
    if menu.adding {
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
        for playlist in actions::matching(view.playlists, |p| p.name.as_str(), &menu.filter) {
            lists = lists.push(
                button(text(playlist.name.as_str()).size(13))
                    .width(Fill)
                    .style(theme::row)
                    .padding([4, 8])
                    .on_press(Message::Row(RowAction::AddTo(playlist.id.clone()))),
            );
        }
        let create = row![
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
        .align_y(Center);
        body = body.push(lists).push(create);
    }
    container(body)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
}

/// Play and Shuffle, and the action strip for the whole set.
pub fn play_buttons<'a>(
    tracks: &'a [TrackMeta],
    anchor: String,
    source: PlaySource,
) -> Element<'a, Message> {
    let enabled = !tracks.is_empty();
    row![
        button(
            row![icon(Icon::Play, 14.0, Tone::Normal), text("Play")]
                .spacing(6)
                .align_y(Center)
        )
        .style(theme::filled)
        .padding([8, 18])
        .on_press_maybe(enabled.then(|| Message::Play(PlayRequest::from(tracks, 0, source)))),
        button(
            row![icon(Icon::Shuffle, 14.0, Tone::Accent), text("Shuffle")]
                .spacing(6)
                .align_y(Center)
        )
        .style(theme::tonal)
        .padding([8, 18])
        .on_press_maybe(
            enabled.then(|| Message::Play(PlayRequest::shuffled(tracks, PlaySource::Shuffle)))
        ),
        button(icon(Icon::More, 18.0, Tone::Accent))
            .style(theme::tonal)
            .padding([8, 10])
            .on_press_maybe(enabled.then(|| {
                Message::Row(RowAction::Open {
                    anchor,
                    tracks: tracks.to_vec(),
                })
            })),
    ]
    .spacing(10)
    .width(Shrink)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(65_400), "1:05");
        assert_eq!(duration(3_723_000), "1:02:03");
    }
}
