//! The player bar's popovers: drawn over the window just above the bar, at
//! its right-hand end; a click anywhere else closes them.

use iced::widget::{
    Column, button, column, container, mouse_area, opaque, pick_list, row, rule, text, text_input,
};
use iced::{Center, Element, Fill, Length, Padding};

use crate::app::{App, Message};
use crate::icons::{Icon, Tone, icon};
use crate::listening::{BarMenu, Msg};
use crate::playback::sleep_timer::{Choice, SleepTimer};
use crate::theme;

/// How far above the window's bottom edge a popover sits: the bar and a gap.
const ABOVE_BAR: f32 = 92.0;

/// The open popover over the whole window, or `None`.
pub fn overlay(app: &App) -> Option<Element<'_, Message>> {
    let menu = app.listening.menu?;
    let card = match menu {
        BarMenu::More => more(app),
        BarMenu::Volume => outputs(app),
    };
    let card = container(card)
        .padding(8)
        .width(Length::Fixed(280.0))
        .style(theme::menu);
    let placed = container(opaque(card))
        .width(Fill)
        .height(Fill)
        .align_right(Fill)
        .align_bottom(Fill)
        .padding(Padding::ZERO.bottom(ABOVE_BAR).right(16.0));
    Some(opaque(
        mouse_area(placed).on_press(Message::Listening(Msg::Menu(None))),
    ))
}

/// One line of a popover.
pub fn item<'a>(
    glyph: Option<Icon>,
    label: impl Into<String>,
    on: Message,
) -> Element<'a, Message> {
    let mut line = row![].spacing(10).align_y(Center);
    if let Some(glyph) = glyph {
        line = line.push(icon(glyph, 16.0, Tone::Secondary));
    }
    button(line.push(text(label.into()).size(14)))
        .width(Fill)
        .padding([6, 10])
        .style(theme::row)
        .on_press(on)
        .into()
}

fn more(app: &App) -> Element<'_, Message> {
    let sleep = |choice: Choice| Message::Listening(Msg::Sleep(choice));
    let mut list = column![
        item(
            Some(Icon::Equalizer),
            "Equaliser…",
            Message::Listening(Msg::OpenEqualizer)
        ),
        item(
            Some(Icon::Expand),
            "Full-Screen Player",
            Message::Listening(Msg::OpenFull)
        ),
        rule::horizontal(1),
        heading("Sleep Timer"),
    ]
    .spacing(2);
    if let Some(timer) = &app.listening.sleep {
        let left = match timer {
            SleepTimer::At { .. } => format!("{} left", timer.label(app.now())),
            _ => timer.label(app.now()),
        };
        list = list.push(item(
            Some(Icon::Close),
            format!("Turn Off ({left})"),
            Message::Listening(Msg::SleepCancel),
        ));
    }
    let armed = app.listening.sleep.as_ref();
    let playing = app.playback.current().is_some();
    list = list
        .push(check_item(
            matches!(armed, Some(SleepTimer::EndOfSong { .. })),
            "End of Song",
            sleep(Choice::EndOfSong),
        ))
        .push(check_item(
            matches!(armed, Some(SleepTimer::EndOfAlbum { .. })),
            "End of Album",
            sleep(Choice::EndOfAlbum),
        ));
    if !playing {
        list = list.push(
            container(
                crate::widgets::secondary("Nothing is playing, so neither end is near.").size(12),
            )
            .padding([0, 10]),
        );
    }
    for minutes in Choice::PRESETS {
        list = list.push(check_item(
            false,
            format!("{minutes} Minutes"),
            sleep(Choice::Minutes(minutes)),
        ));
    }
    let typed = &app.listening.sleep_minutes;
    let valid = crate::playback::sleep_timer::parse_minutes(typed).is_some();
    list = list.push(
        container(
            row![
                text_input("Minutes", typed)
                    .on_input(|s| Message::Listening(Msg::SleepMinutes(s)))
                    .on_submit(Message::Listening(Msg::SleepCustom))
                    .padding(6)
                    .size(13)
                    .style(theme::field),
                button(text("Set").size(13))
                    .style(theme::tonal)
                    .padding([6, 12])
                    .on_press_maybe(valid.then_some(Message::Listening(Msg::SleepCustom))),
            ]
            .spacing(6)
            .align_y(Center),
        )
        .padding([4, 10]),
    );
    list.into()
}

/// What the player bar says under the song: the sleep timer while it is
/// set, with a way to turn it off.
pub fn status_line(app: &App) -> Option<Element<'_, Message>> {
    let timer = app.listening.sleep.as_ref()?;
    let chip = row![
        icon(Icon::Moon, 12.0, Tone::Accent),
        text(timer.label(app.now()))
            .size(11)
            .style(|t: &iced::Theme| text::Style {
                color: Some(theme::accent(t)),
            }),
        button(icon(Icon::Close, 10.0, Tone::Secondary))
            .padding(2)
            .style(theme::row)
            .on_press(Message::Listening(Msg::SleepCancel)),
    ]
    .spacing(4)
    .align_y(Center);
    Some(chip.into())
}

/// A bar button that opens one of its popovers.
pub fn menu_button<'a>(glyph: Icon, open: bool, menu: BarMenu) -> Element<'a, Message> {
    button(icon(
        glyph,
        18.0,
        if open { Tone::Accent } else { Tone::Normal },
    ))
    .padding(6)
    .style(if open { theme::icon_on } else { theme::row })
    .on_press(Message::Listening(Msg::Menu(Some(menu))))
    .into()
}

/// A line with a tick in front when it is the one chosen.
pub fn check_item<'a>(
    on: bool,
    label: impl Into<String>,
    message: Message,
) -> Element<'a, Message> {
    let tick = container(if on {
        icon(Icon::Check, 14.0, Tone::Accent)
    } else {
        iced::widget::space().into()
    })
    .width(Length::Fixed(16.0));
    button(
        row![tick, text(label.into()).size(14)]
            .spacing(8)
            .align_y(Center),
    )
    .width(Fill)
    .padding([6, 10])
    .style(theme::row)
    .on_press(message)
    .into()
}

fn heading<'a>(label: &'a str) -> Element<'a, Message> {
    container(
        crate::widgets::secondary(label)
            .size(12)
            .font(crate::widgets::SEMIBOLD),
    )
    .padding([4, 10])
    .into()
}

/// The volume button's popover: where the sound goes.
fn outputs(app: &App) -> Element<'_, Message> {
    let chosen = &app.config.sound.device;
    let listed = match &app.listening.devices {
        Some(Ok(listed)) => listed.as_slice(),
        _ => &[],
    };
    let mut list = Column::new().spacing(2).push(heading("Play On"));
    for option in crate::sound::device_options(listed, chosen) {
        let on = crate::sound::chosen_option(std::slice::from_ref(&option), chosen).is_some();
        list = list.push(check_item(
            on,
            option.label,
            Message::Listening(Msg::ChooseDevice(option.choice)),
        ));
    }
    match &app.listening.devices {
        None => list = list.push(heading("Looking for devices…")),
        Some(Err(error)) => {
            list = list.push(heading("The devices couldn't be listed:"));
            list = list.push(container(text(error.as_str()).size(12)).padding([0, 10]));
        }
        Some(Ok(_)) => {}
    }
    if app.playback.device_fell_back() {
        list = list.push(
            container(
                crate::widgets::secondary(
                    "The chosen device couldn't be opened, so the system default is playing.",
                )
                .size(12),
            )
            .padding([4, 10]),
        );
    }
    list.into()
}

/// The output picker in Settings → Sound.
pub fn device_row<'a>(extras: &crate::listening::SettingsExtras<'a>) -> Element<'a, Message> {
    let chosen = &extras.config.sound.device;
    let listed = match &extras.listening.devices {
        Some(Ok(listed)) => listed.as_slice(),
        _ => &[],
    };
    let options = crate::sound::device_options(listed, chosen);
    let selected = crate::sound::chosen_option(&options, chosen);
    let mut words = column![text("Output")].spacing(2).width(Fill);
    if extras.device_fell_back {
        words = words.push(
            crate::widgets::secondary(
                "The chosen device couldn't be opened, so the system default is playing.",
            )
            .size(13),
        );
    } else if let Some(Err(error)) = &extras.listening.devices {
        words = words.push(
            crate::widgets::secondary(format!("The devices couldn't be listed: {error}")).size(13),
        );
    }
    row![
        words,
        pick_list(options, selected, |o: crate::sound::DeviceOption| {
            Message::Listening(Msg::ChooseDevice(o.choice))
        })
        .on_open(Message::Listening(Msg::LoadDevices))
        .padding([6, 10])
        .text_size(14)
        .width(Length::Fixed(360.0)),
    ]
    .spacing(10)
    .align_y(Center)
    .into()
}

/// The player bar's cover, which opens the full-screen player.
pub fn open_full<'a>(art: Element<'a, Message>) -> Element<'a, Message> {
    button(art)
        .padding(0)
        .style(theme::row)
        .on_press(Message::Listening(Msg::OpenFull))
        .into()
}
