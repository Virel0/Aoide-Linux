//! Get Info: everything the server knows about one song's file
//! (`item_details`), in a sheet over the window, opened from any song's
//! actions. Every line the server could not say is left out.

use std::time::{SystemTime, UNIX_EPOCH};

use aoide_jellyfin::ItemDetails;
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A day as "14 Mar 2026", in UTC (the server's dates are).
pub fn date(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// "31.8 MB", "640 KB".
pub fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes.div_ceil(1_000))
    }
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
        item.index_number.map(|n| match item.parent_index_number {
            Some(disc) => format!("{n} (disc {disc})"),
            None => n.to_string(),
        }),
    );
    add("Year", item.production_year.map(|y| y.to_string()));
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
        match (&details.container, &details.codec) {
            (Some(container), Some(codec)) if container != codec => {
                Some(format!("{} ({})", codec.to_uppercase(), container))
            }
            (_, Some(codec)) => Some(codec.to_uppercase()),
            (Some(container), None) => Some(container.to_uppercase()),
            (None, None) => None,
        },
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
        match (details.channels, &details.channel_layout) {
            (Some(n), Some(layout)) => Some(format!("{n} ({layout})")),
            (Some(n), None) => Some(n.to_string()),
            (None, Some(layout)) => Some(layout.clone()),
            (None, None) => None,
        },
    );
    add("Size", details.size.map(size));
    add("Date added", details.date_added.map(date));
    add(
        "Plays",
        Some(match details.play_count {
            1 => "1 (counted when it starts)".to_owned(),
            n => format!("{n} (counted when they start)"),
        }),
    );
    add("Last played", details.last_played.map(date));
    add("File", details.path.clone());
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
                        Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
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
    fn dates_and_sizes_read_plainly() {
        // 2026-03-14T09:30:00Z
        assert_eq!(
            date(UNIX_EPOCH + Duration::from_secs(1_773_480_600)),
            "14 Mar 2026"
        );
        assert_eq!(date(UNIX_EPOCH), "1 Jan 1970");
        assert_eq!(size(31_754_112), "31.8 MB");
        assert_eq!(size(640_001), "641 KB");
    }

    #[test]
    fn the_sheet_says_what_the_server_knows_and_leaves_out_the_rest() {
        let item = serde_json::from_value(serde_json::json!({
            "Id": "t1", "Name": "Lantern", "Type": "Audio", "Artists": ["Mara Vey"],
            "Album": "Saltwater", "IndexNumber": 5, "ParentIndexNumber": 1,
            "RunTimeTicks": 2_000_000_000_i64, "ProductionYear": 2022
        }))
        .unwrap();
        let details = ItemDetails {
            item,
            path: Some("/srv/music/Saltwater/05 Lantern.flac".into()),
            container: Some("flac".into()),
            codec: Some("flac".into()),
            bitrate: Some(1_043_201),
            sample_rate: Some(44_100),
            bit_depth: Some(24),
            channels: Some(2),
            channel_layout: Some("stereo".into()),
            size: Some(31_754_112),
            date_added: None,
            play_count: 3,
            last_played: None,
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
        assert_eq!(find("Date added"), None, "unknown is left out");
        assert_eq!(find("Plays"), Some("3 (counted when they start)"));
    }
}
