//! A person's profile picture, small and round, or their name's first letter
//! in a round tile while there is none.

use iced::widget::{container, image, text};
use iced::{ContentFit, Element, Theme, border};

use crate::app::Message;
use crate::artwork::Artwork;
use crate::{avatars, theme};

/// `name`'s picture from `path` (an `avatar_url`) at `size`, or their
/// initial until it arrives or where there is none.
pub fn avatar<'a>(
    name: &str,
    path: Option<&str>,
    artwork: &'a Artwork,
    size: f32,
) -> Element<'a, Message> {
    let picture = path.and_then(|path| artwork.get(&avatars::key(path)));
    match picture {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(size / 2.0)
            .into(),
        None => container(
            text(initial(name))
                .size(size * 0.45)
                .font(super::SEMIBOLD)
                .style(|t| text::Style {
                    color: Some(theme::secondary(t)),
                }),
        )
        .center(size)
        .style(move |t| round(t, size))
        .into(),
    }
}

/// The first letter of `name`, as a capital; `?` for a blank name.
pub fn initial(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// The placeholder's ground, as a circle.
fn round(theme: &Theme, size: f32) -> container::Style {
    container::Style {
        border: border::rounded(size / 2.0),
        ..theme::placeholder(theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_initial_is_the_names_first_letter_as_a_capital() {
        assert_eq!(initial("robin"), "R");
        assert_eq!(initial("  émile"), "É");
        assert_eq!(initial(""), "?");
    }
}
