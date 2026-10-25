//! The player bar along the bottom: what is playing, the transport, the seek
//! bar, and the switches that govern what plays next.

use aoide_audio::RepeatMode;
use iced::widget::{button, column, container, row, slider, text};
use iced::{Center, Element, Fill, Length};

use crate::app::{App, Message, Panel, Signed, Transport};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::{theme, widgets};

const HEIGHT: f32 = 84.0;

fn control<'a>(glyph: Icon, size: f32, on: bool, message: Transport) -> Element<'a, Message> {
    button(icon(
        glyph,
        size,
        if on { Tone::Accent } else { Tone::Normal },
    ))
    .padding(6)
    .style(if on { theme::icon_on } else { theme::row })
    .on_press(Message::Player(message))
    .into()
}

pub fn view<'a>(app: &'a App, signed: &'a Signed) -> Element<'a, Message> {
    let playback = &app.playback;
    let current = playback.current().map(|(_, q)| &q.meta);

    let now = row![
        widgets::art(
            current
                .and_then(|m| m.artwork.as_ref())
                .map(|(i, t)| ArtKey::new(i, t))
                .as_ref(),
            &app.artwork,
            Length::Fixed(56.0),
            6.0,
        ),
        column![
            text(current.map_or("Not Playing", |m| m.title.as_str()))
                .size(14)
                .font(widgets::SEMIBOLD)
                .wrapping(text::Wrapping::None),
            widgets::secondary(current.map_or("", |m| m.artist.as_str()))
                .size(12)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2)
        .clip(true),
    ]
    .spacing(12)
    .align_y(Center)
    .width(Length::Fixed(300.0));

    let repeat = playback.repeat();
    let transport = row![
        control(
            Icon::Shuffle,
            18.0,
            playback.is_shuffled(),
            Transport::Shuffle
        ),
        control(Icon::Previous, 22.0, false, Transport::Previous),
        control(
            if playback.is_playing() {
                Icon::Pause
            } else {
                Icon::Play
            },
            28.0,
            false,
            Transport::Toggle,
        ),
        control(Icon::Next, 22.0, false, Transport::Next),
        control(
            if repeat == RepeatMode::One {
                Icon::RepeatOne
            } else {
                Icon::Repeat
            },
            18.0,
            repeat != RepeatMode::Off,
            Transport::Repeat,
        ),
    ]
    .spacing(8)
    .align_y(Center);

    let length = current.and_then(|m| m.duration_ms).unwrap_or(0).max(0) as u64;
    let at = signed
        .scrub
        .unwrap_or(playback.position_ms())
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
    let clock = |s: String| widgets::secondary(s).size(11).width(44);
    let scrubber = row![
        clock(widgets::duration(at)),
        seek,
        clock(format!("-{}", widgets::duration(length.saturating_sub(at))))
    ]
    .spacing(8)
    .align_y(Center);
    let middle = column![container(transport).center_x(Fill), scrubber]
        .spacing(4)
        .width(Fill)
        .max_width(620);

    let settings = &app.config.settings;
    let panel_on = |p: Panel| signed.panel == Some(p);
    let tools = row![
        control(
            Icon::Crossfade,
            18.0,
            settings.crossfade,
            Transport::Crossfade
        ),
        control(Icon::Infinity, 20.0, settings.infinity, Transport::Infinity),
        control(
            Icon::Queue,
            18.0,
            panel_on(Panel::Queue),
            Transport::Panel(Panel::Queue)
        ),
        control(
            Icon::Lyrics,
            18.0,
            panel_on(Panel::Lyrics),
            Transport::Panel(Panel::Lyrics)
        ),
        icon(Icon::Volume, 18.0, Tone::Secondary),
        slider(0.0..=1.0, playback.volume(), |v| Message::Player(
            Transport::Volume(v)
        ))
        .step(0.01_f32)
        .width(100)
        .style(theme::scrubber),
        crate::player_menu::menu_button(
            Icon::More,
            app.listening.menu == Some(crate::listening::BarMenu::More),
            crate::listening::BarMenu::More
        ),
    ]
    .spacing(6)
    .align_y(Center)
    .width(Length::Fixed(360.0));

    container(
        row![now, container(middle).center_x(Fill), tools]
            .spacing(16)
            .align_y(Center)
            .padding([0, 16]),
    )
    .height(Length::Fixed(HEIGHT))
    .center_y(Length::Fixed(HEIGHT))
    .style(theme::bar)
    .into()
}
