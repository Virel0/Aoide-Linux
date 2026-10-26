//! The pieces every screen draws with: the fonts and secondary text here;
//! artwork, tiles and shelves, a page's frame and states, track rows and
//! the action strip under a row in the modules below.

use iced::widget::{Row, row, text, toggler};
use iced::{Center, Fill, Font, font};

use crate::app::Message;
use crate::theme;

mod art;
mod menu_strip;
mod page;
mod tiles;
mod tracks;

pub use art::art;
pub use menu_strip::menu_strip;
pub use page::{PAGE, empty, failure, loading, page, plain_failure, title_bar};
pub use tiles::{shelf, tile_grid};
pub use tracks::{
    ListStyle, play_buttons, play_buttons_with, track_list, track_list_with_headings, track_row,
};

/// Inter, bundled (`fonts/`, under the SIL Open Font License), so text looks
/// the same on every desktop rather than whatever the system's sans-serif
/// fallback is: on macOS that left bold headings in a monospace face.
pub const FAMILY: &str = "Inter";

/// Every text not given another font.
pub const REGULAR: Font = Font::with_name(FAMILY);

pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..REGULAR
};

/// Inter is bundled in Regular and Bold only; this one is drawn in Bold.
pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..REGULAR
};

/// The font files, loaded before the window opens.
pub const FONTS: [&[u8]; 2] = [
    include_bytes!("../fonts/Inter-Regular.ttf"),
    include_bytes!("../fonts/Inter-Bold.ttf"),
];

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

/// A setting's switch: the label on the left and the switch on the right,
/// as macOS lays them out.
pub fn switch<'a>(
    label: impl text::IntoFragment<'a>,
    on: bool,
    toggle: impl Fn(bool) -> Message + 'a,
) -> Row<'a, Message> {
    row![
        text(label).width(Fill),
        toggler(on).style(theme::switch).on_toggle(toggle),
    ]
    .spacing(12)
    .align_y(Center)
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
