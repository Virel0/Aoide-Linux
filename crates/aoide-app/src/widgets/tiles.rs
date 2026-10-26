//! Album, artist and playlist tiles: in a grid that fills the width, or on
//! a shelf scrolled sideways.

use iced::widget::{Column, button, column, container, grid, mouse_area, row, scrollable, text};
use iced::{Center, Element, Fill, Length};

use super::art::{art, round_art};
use super::{BOLD, SEMIBOLD};
use crate::actions::{Collection, RowAction};
use crate::app::Message;
use crate::artwork::Artwork;
use crate::ellipsis::ellipsis;
use crate::icons::{Icon, Tone, icon};
use crate::library::Tile;
use crate::screens::Route;
use crate::theme;

/// A tile's picture: an artist's round, anything else's square.
fn tile_art<'a>(tile: &'a Tile, artwork: &'a Artwork, size: Length) -> Element<'a, Message> {
    if tile.round {
        container(round_art(tile.art.as_ref(), artwork))
            .width(size)
            .height(size)
            .into()
    } else {
        art(tile.art.as_ref(), artwork, size, 8.0)
    }
}

/// What a tile's `…` (or a right click on it) opens: the album's or
/// artist's actions.
fn tile_menu(tile: &Tile, route: &Route) -> Option<Message> {
    let collection = match route {
        Route::Album { id, .. } => Collection::Album(id.clone()),
        Route::Artist { id, .. } => Collection::Artist(id.clone()),
        Route::Playlist { id } => Collection::Playlist(id.clone()),
        _ => return None,
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
        ellipsis(tile.title.as_str()).size(14).font(SEMIBOLD),
        ellipsis(tile.subtitle.as_str())
            .size(13)
            .color(|t| Some(theme::secondary(t))),
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
    column![tile_art(tile, artwork, art_size), tile_caption(tile, menu),]
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
            container(tile_art(tile, artwork, Fill)).height(Fill),
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
