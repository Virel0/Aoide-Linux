//! Covers and artists' pictures, with a placeholder until they arrive.

use iced::widget::{column, container, image, responsive, row, space};
use iced::{ContentFit, Element, Fill, Length, border};

use crate::app::Message;
use crate::artwork::{ArtKey, Artwork};
use crate::covers::Cover;
use crate::icons::{Icon, Tone, icon};
use crate::theme;

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

/// A playlist's cover at `size`: one picture, or four albums' in a 2×2
/// mosaic whose outer corners are rounded; a quiet square while there is
/// none yet.
pub fn cover<'a>(
    cover: Option<&Cover>,
    artwork: &'a Artwork,
    size: f32,
    radius: f32,
) -> Element<'a, Message> {
    let [a, b, c, d] = match cover {
        Some(Cover::Mosaic(keys)) => keys.as_ref(),
        Some(Cover::One(key)) => return art(Some(key), artwork, size, radius),
        None => return art(None, artwork, size, radius),
    };
    let half = size / 2.0;
    let cell = |key: &ArtKey, corner| quarter(artwork.get(key), half, corner);
    column![
        row![
            cell(a, border::top_left(radius)),
            cell(b, border::top_right(radius))
        ],
        row![
            cell(c, border::bottom_left(radius)),
            cell(d, border::bottom_right(radius))
        ],
    ]
    .into()
}

/// One cover of a mosaic, rounded at its outer corner only.
fn quarter<'a>(
    handle: Option<&image::Handle>,
    side: f32,
    corner: border::Radius,
) -> Element<'a, Message> {
    match handle {
        Some(handle) => image(handle.clone())
            .width(side)
            .height(side)
            .content_fit(ContentFit::Cover)
            .border_radius(corner)
            .into(),
        None => container(space())
            .width(side)
            .height(side)
            .style(move |t| {
                let mut style = theme::placeholder(t);
                style.border.radius = corner;
                style
            })
            .into(),
    }
}

/// A playlist's cover in a small square; where it has none, a placeholder
/// with the playlist's glyph.
pub fn playlist_art<'a>(
    found: Option<&Cover>,
    artwork: &'a Artwork,
    size: f32,
    glyph: Icon,
) -> Element<'a, Message> {
    let radius = (size / 6.0).round();
    if found.is_some() {
        return cover(found, artwork, size, radius);
    }
    container(icon(glyph, (size * 0.6).round(), Tone::Secondary))
        .center(size)
        .style(move |t| {
            let mut style = theme::placeholder(t);
            style.border.radius = radius.into();
            style
        })
        .into()
}

/// An artist's picture, round and centred in the space it is given; a round
/// placeholder with the artist glyph where there is no picture yet.
pub fn round_art<'a>(key: Option<&'a ArtKey>, artwork: &'a Artwork) -> Element<'a, Message> {
    responsive(move |space| {
        let side = space.width.min(space.height).max(0.0);
        let picture: Element<'a, Message> = match key.and_then(|key| artwork.get(key)) {
            Some(handle) => image(handle.clone())
                .width(side)
                .height(side)
                .content_fit(ContentFit::Cover)
                .border_radius(side / 2.0)
                .into(),
            None => container(icon(Icon::Artist, (side * 0.4).max(1.0), Tone::Secondary))
                .center(side)
                .style(move |t| {
                    let mut style = theme::placeholder(t);
                    style.border.radius = (side / 2.0).into();
                    style
                })
                .into(),
        };
        container(picture).center(Fill).into()
    })
    .into()
}
