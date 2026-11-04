//! The frosted card: the cover (with its motion cover over it), the song,
//! the seek bar and the transport. Upright on its own or beside the lyrics;
//! wide, with the song large, in the wide layout.

use iced::widget::{column, container, row, slider, space, text};
use iced::{Center, Element, Fill, Length};

use crate::app::{App, Message, Signed, Transport};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

/// The upright card's width.
const CARD: f32 = 420.0;
/// The cover's side in the upright card, and in the wide one.
const COVER: f32 = 356.0;
const WIDE_COVER: f32 = 300.0;

/// The upright card.
pub fn card<'a>(
    app: &'a App,
    signed: &'a Signed,
    current: Option<&'a TrackMeta>,
) -> Element<'a, Message> {
    let words = column![
        text(title(current))
            .size(22)
            .font(widgets::BOLD)
            .width(Fill)
            .align_x(Center),
        widgets::secondary(byline(current))
            .size(15)
            .width(Fill)
            .align_x(Center),
    ]
    .spacing(4);
    let body = column![
        cover(app, current, COVER),
        words,
        scrubber(app, signed, current),
        transport(&app.playback),
    ]
    .spacing(18)
    .align_x(Center);
    container(body)
        .padding(32)
        .width(CARD)
        .style(theme::glass)
        .into()
}

/// The wide card: the cover on the left, the song large beside it.
pub fn wide<'a>(
    app: &'a App,
    signed: &'a Signed,
    current: Option<&'a TrackMeta>,
) -> Element<'a, Message> {
    let words = column![
        text(title(current)).size(46).font(widgets::BOLD),
        widgets::secondary(byline(current)).size(24),
        space().height(20),
        container(scrubber(app, signed, current)).max_width(620.0),
        transport(&app.playback),
    ]
    .spacing(8)
    .width(Fill);
    container(
        row![cover(app, current, WIDE_COVER), words]
            .spacing(56)
            .align_y(Center),
    )
    .padding(56)
    .width(Fill)
    .max_width(1600.0)
    .style(theme::glass)
    .into()
}

fn title(current: Option<&TrackMeta>) -> &str {
    current.map_or("Not playing", |m| m.title.as_str())
}

/// The artist, and the album after them when there is one.
fn byline(current: Option<&TrackMeta>) -> String {
    current.map_or_else(String::new, |m| {
        if m.album.is_empty() {
            m.artist.clone()
        } else {
            format!("{} \u{2014} {}", m.artist, m.album)
        }
    })
}

/// The cover, `side` square, with the album's motion cover over it.
fn cover<'a>(app: &'a App, current: Option<&'a TrackMeta>, side: f32) -> Element<'a, Message> {
    let key = current
        .and_then(|m| m.artwork.as_ref())
        .map(|(item, tag)| ArtKey::new(item, tag));
    let size = Length::Fixed(side);
    let still = widgets::art(key.as_ref(), &app.artwork, size, 14.0);
    let handle = key.as_ref().and_then(|key| app.artwork.get(key));
    let album = current.and_then(|m| m.album_id.as_deref());
    crate::motion::over(&app.motion, album, still, handle, size, 14.0)
}

/// The seek bar between the time played and the time left.
fn scrubber<'a>(app: &App, signed: &Signed, current: Option<&TrackMeta>) -> Element<'a, Message> {
    let length = current.and_then(|m| m.duration_ms).unwrap_or(0).max(0) as u64;
    let at = signed
        .scrub
        .unwrap_or(app.playback.position_ms())
        .min(length.max(1));
    let seek: Element<'a, Message> = if length > 0 {
        slider(0.0..=length as f32, at as f32, |v| {
            Message::Player(Transport::Scrub(v))
        })
        .on_release(Message::Player(Transport::ScrubEnd))
        .step(250.0_f32)
        .style(theme::scrubber)
        .into()
    } else {
        slider(0.0..=1.0, 0.0, |_| Message::Ignore)
            .style(theme::scrubber)
            .into()
    };
    let clock = |s: String| widgets::secondary(s).size(12).width(44);
    row![
        clock(widgets::duration(at)),
        seek,
        clock(format!("-{}", widgets::duration(length.saturating_sub(at))))
    ]
    .spacing(10)
    .align_y(Center)
    .width(Fill)
    .into()
}

/// Shuffle, Previous, Play or Pause, Next and Repeat.
fn transport<'a>(playback: &crate::playback::Playback) -> Element<'a, Message> {
    let control = |glyph: Icon, size: f32, on: bool, message: Transport| {
        iced::widget::button(icon(
            glyph,
            size,
            if on { Tone::Accent } else { Tone::Normal },
        ))
        .padding(8)
        .style(if on { theme::icon_on } else { theme::row })
        .on_press(Message::Player(message))
    };
    let repeat = playback.repeat();
    let play = if playback.is_playing() {
        Icon::Pause
    } else {
        Icon::Play
    };
    let repeat_icon = if repeat == aoide_audio::RepeatMode::One {
        Icon::RepeatOne
    } else {
        Icon::Repeat
    };
    row![
        control(
            Icon::Shuffle,
            18.0,
            playback.is_shuffled(),
            Transport::Shuffle
        ),
        control(Icon::Previous, 24.0, false, Transport::Previous),
        control(play, 36.0, false, Transport::Toggle),
        control(Icon::Next, 24.0, false, Transport::Next),
        control(
            repeat_icon,
            18.0,
            repeat != aoide_audio::RepeatMode::Off,
            Transport::Repeat
        ),
    ]
    .spacing(14)
    .align_y(Center)
    .into()
}
