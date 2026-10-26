//! Covers and artists' pictures, with a placeholder until they arrive.

use iced::widget::{container, image, responsive, space};
use iced::{ContentFit, Element, Fill, Length};

use crate::app::Message;
use crate::artwork::{ArtKey, Artwork};
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
