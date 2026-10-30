//! Get Info: what Melete knows about one song and its file
//! (`item_details`), in a sheet over the window, opened from any song's
//! actions. Every line the server could not say is left out.

use aoide_melete::ItemDetails;
use iced::widget::{
    Column, button, center, column, container, mouse_area, opaque, row, scrollable, text,
};
use iced::{Element, Fill, Task};

use crate::app::{App, Message};
use crate::icons::{Icon, Tone, icon};
use crate::screens::Load;
use crate::{theme, widgets};

/// The sheet: the song's title while its details load.
#[derive(Debug, Clone)]
pub struct Sheet {
    /// Which song: a late answer for another is dropped.
    pub id: String,
    pub title: String,
    pub details: Load<Box<ItemDetails>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Open { id: String, title: String },
    Loaded(String, Result<Box<ItemDetails>, crate::failure::Failure>),
    Close,
}

/// The sheet's lines: label and value, in the order they read best.
pub fn lines(details: &ItemDetails) -> Vec<(&'static str, String)> {
    let item = &details.item;
    let mut lines = Vec::new();
    let mut add = |label: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            lines.push((label, value));
        }
    };
    add("Title", Some(item.name.clone()));
    add(
        "Artist",
        (!item.artists.is_empty()).then(|| item.artists.join(", ")),
    );
    add("Album artist", item.album_artist.clone());
    add("Album", item.album.clone());
    add(
        "Track",
        item.track_number.map(|n| match item.disc_number {
            Some(disc) => format!("{n} (disc {disc})"),
            None => n.to_string(),
        }),
    );
    add("Year", item.year.map(|y| y.to_string()));
    add(
        "Genre",
        (!item.genres.is_empty()).then(|| item.genres.join(", ")),
    );
    add(
        "Length",
        item.duration()
            .map(|d| widgets::duration(d.as_millis() as u64)),
    );
    add(
        "Format",
        details.codec.as_ref().map(|codec| codec.to_uppercase()),
    );
    add(
        "Bit rate",
        details
            .bitrate
            .map(|b| format!("{} kbps", (b + 500) / 1_000)),
    );
    add(
        "Sample rate",
        details.sample_rate.map(|r| {
            let khz = f64::from(r) / 1_000.0;
            if khz.fract() == 0.0 {
                format!("{khz:.0} kHz")
            } else {
                format!("{khz:.1} kHz")
            }
        }),
    );
    add("Bit depth", details.bit_depth.map(|d| format!("{d}-bit")));
    add(
        "Channels",
        details.channels.map(|n| match n {
            1 => "1 (mono)".to_owned(),
            2 => "2 (stereo)".to_owned(),
            n => n.to_string(),
        }),
    );
    add("ISRC", item.isrc.clone());
    lines
}

impl App {
    pub(crate) fn info(&mut self, message: Msg) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::Open { id, title } => {
                signed.menu = None;
                signed.info = Some(Sheet {
                    id: id.clone(),
                    title,
                    details: Load::Loading,
                });
                let client = signed.connection.client.clone();
                Task::perform(
                    async move {
                        let result = client.item_details(&id).await;
                        (id, result)
                    },
                    |(id, result)| match result {
                        Err(aoide_melete::Error::Unauthorized) => Message::TokenRejected,
                        result => Message::Info(Msg::Loaded(
                            id,
                            result
                                .map(Box::new)
                                .map_err(|e| crate::failure::describe(&e)),
                        )),
                    },
                )
            }
            Msg::Loaded(id, result) => {
                if let Some(sheet) = signed.info.as_mut().filter(|s| s.id == id) {
                    sheet.details = Load::from(result);
                }
                Task::none()
            }
            Msg::Close => {
                signed.info = None;
                Task::none()
            }
        }
    }
}

/// The sheet over everything; a click outside closes it.
pub fn view(sheet: &Sheet, details: bool) -> Element<'_, Message> {
    let close = Message::Info(Msg::Close);
    let mut body = column![row![
        text(sheet.title.as_str())
            .size(20)
            .font(widgets::BOLD)
            .width(Fill),
        button(icon(Icon::Close, 14.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(close.clone()),
    ]]
    .spacing(14);
    body = body.push(match &sheet.details {
        Load::Loading => widgets::loading(),
        Load::Failed(error) => widgets::failure(
            error,
            Some(Message::Info(Msg::Open {
                id: sheet.id.clone(),
                title: sheet.title.clone(),
            })),
            details,
        ),
        Load::Ready(details) => {
            let mut list = Column::new().spacing(8);
            for (label, value) in lines(details) {
                list = list.push(
                    row![
                        container(widgets::secondary(label).size(13)).width(120),
                        text(value).size(13).width(Fill),
                    ]
                    .spacing(12),
                );
            }
            scrollable(list).height(iced::Length::Shrink).into()
        }
    });
    let card = container(body)
        .padding(20)
        .max_width(560)
        .max_height(620)
        .style(theme::menu);
    opaque(mouse_area(center(opaque(card)).style(theme::scrim)).on_press(close))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_sheet_says_what_the_server_knows_and_leaves_out_the_rest() {
        let item = aoide_melete::Item {
            id: "1".into(),
            name: "Lantern".into(),
            artists: vec!["Mara Vey".into()],
            album: Some("Saltwater".into()),
            track_number: Some(5),
            disc_number: Some(1),
            duration: Some(Duration::from_secs(200)),
            year: Some(2022),
            ..aoide_melete::Item::default()
        };
        let details = ItemDetails {
            item,
            codec: Some("flac".into()),
            bitrate: Some(1_043_201),
            sample_rate: Some(44_100),
            bit_depth: Some(24),
            channels: Some(2),
        };
        let lines = lines(&details);
        let find = |label: &str| {
            lines
                .iter()
                .find(|(l, _)| *l == label)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(find("Format"), Some("FLAC"));
        assert_eq!(find("Bit rate"), Some("1043 kbps"));
        assert_eq!(find("Sample rate"), Some("44.1 kHz"));
        assert_eq!(find("Channels"), Some("2 (stereo)"));
        assert_eq!(find("Track"), Some("5 (disc 1)"));
        assert_eq!(find("Length"), Some("3:20"));
        assert_eq!(find("ISRC"), None, "unknown is left out");
    }
}
