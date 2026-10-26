//! Track lists and their rows, and the Play and Shuffle buttons above them.

use aoide_store::PlaySource;
use iced::widget::{Column, button, column, container, row, text};
use iced::{Center, Element, Fill, Length};

use super::art::art;
use super::menu_strip::menu_strip;
use super::{SEMIBOLD, duration, secondary};
use crate::actions::{PlayRequest, RowAction};
use crate::app::Message;
use crate::artwork::ArtKey;
use crate::ellipsis::ellipsis;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::screens::View;
use crate::theme;

/// How a track list looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListStyle {
    /// Track numbers instead of covers: an album's own list.
    pub numbered: bool,
    pub show_album: bool,
}

/// A list of tracks. A click plays the list from that track; the `…` opens
/// the action strip under the row.
pub fn track_list<'a>(
    tracks: &'a [TrackMeta],
    view: &View<'a>,
    style: ListStyle,
    source: PlaySource,
) -> Element<'a, Message> {
    track_list_with_headings(tracks, view, style, source, &[])
}

/// A track list with a heading before some rows: `(index, heading)`, in
/// order, such as "Disc 2" before the first track of the second disc.
pub fn track_list_with_headings<'a>(
    tracks: &'a [TrackMeta],
    view: &View<'a>,
    style: ListStyle,
    source: PlaySource,
    headings: &[(usize, String)],
) -> Element<'a, Message> {
    let mut list = Column::with_capacity(tracks.len()).spacing(2);
    let mut headings = headings.iter().peekable();
    for (index, track) in tracks.iter().enumerate() {
        while let Some((_, heading)) = headings.next_if(|(at, _)| *at <= index) {
            list = list.push(
                container(text(heading.clone()).size(15).font(SEMIBOLD)).padding(
                    iced::Padding::new(0.0)
                        .top(if index == 0 { 0.0 } else { 14.0 })
                        .bottom(4.0)
                        .left(8.0),
                ),
            );
        }
        let anchor = format!("{}:{index}", view.key);
        list = list.push(track_row(track, &anchor, view, style, move || {
            Message::Play(PlayRequest::from(tracks, index, source))
        }));
        if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
            list = list.push(menu_strip(menu, view));
        }
    }
    list.into()
}

/// A row's second line. An album's own list names the artist only where it
/// is not the album's artist, as Apple Music does; other lists always name
/// it, and the album too when they say so.
fn subtitle(track: &TrackMeta, style: ListStyle) -> Option<String> {
    if style.numbered && !style.show_album {
        let same = track
            .album_artist
            .as_deref()
            .is_some_and(|a| a.trim().eq_ignore_ascii_case(track.artist.trim()));
        return (!same && !track.artist.is_empty()).then(|| track.artist.clone());
    }
    Some(if style.show_album && !track.album.is_empty() {
        format!("{} · {}", track.artist, track.album)
    } else {
        track.artist.clone()
    })
}

/// A title and a line under it, at the sizes a track row uses.
const TWO_LINES: f32 = 34.0;

/// One track row.
pub fn track_row<'a>(
    track: &'a TrackMeta,
    anchor: &str,
    view: &View<'a>,
    style: ListStyle,
    play: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    let playing = view.playing == Some(track.id.as_str());
    let favourite = view
        .favourites
        .get(&track.id)
        .copied()
        .unwrap_or(track.favourite);
    let anchor = anchor.to_owned();
    let more = button(icon(Icon::More, 18.0, Tone::Secondary))
        .style(theme::row)
        .padding(6)
        .on_press_with(move || {
            Message::Row(RowAction::Open {
                anchor: anchor.clone(),
                tracks: vec![track.clone()],
            })
        });
    let body = row![
        track_lead(track, view, style, playing),
        track_lines(track, style, playing)
    ]
    .spacing(12)
    .align_y(Center);
    let body = if favourite {
        body.push(icon(Icon::HeartFilled, 14.0, Tone::Accent))
    } else {
        body
    };
    let body = body
        .push(
            secondary(
                track
                    .duration_ms
                    .map(|ms| duration(ms.max(0) as u64))
                    .unwrap_or_default(),
            )
            .size(13),
        )
        .push(more);
    button(body)
        .padding([4, 8])
        .width(Fill)
        .style(if playing {
            theme::row_selected
        } else {
            theme::row
        })
        .on_press_with(play)
        .into()
}

/// What starts a row: the track number (a glyph while it plays) in an
/// album's own list, the cover anywhere else.
fn track_lead<'a>(
    track: &'a TrackMeta,
    view: &View<'a>,
    style: ListStyle,
    playing: bool,
) -> Element<'a, Message> {
    if style.numbered {
        let label: Element<'a, Message> = if playing {
            icon(Icon::Song, 14.0, Tone::Accent)
        } else {
            secondary(
                track
                    .track_number
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            )
            .size(13)
            .into()
        };
        container(label).width(28).align_x(Center).into()
    } else {
        art(
            track
                .artwork
                .as_ref()
                .map(|(i, t)| ArtKey::new(i, t))
                .as_ref(),
            view.artwork,
            Length::Fixed(40.0),
            4.0,
        )
    }
}

/// A row's title and the line under it.
fn track_lines<'a>(track: &'a TrackMeta, style: ListStyle, playing: bool) -> Element<'a, Message> {
    let mut lines = column![
        ellipsis(track.title.as_str())
            .size(14)
            .color(move |t| playing.then(|| theme::accent(t))),
    ];
    if let Some(subtitle) = subtitle(track, style) {
        lines = lines.push(
            ellipsis(subtitle)
                .size(12)
                .color(|t| Some(theme::secondary(t))),
        );
    }
    // Every row as tall as one with two lines, the title centred when it is
    // alone: an album's rows stay even where only some name an artist.
    container(lines.spacing(2).clip(true))
        .width(Fill)
        .height(Length::Fixed(TWO_LINES))
        .align_y(Center)
        .clip(true)
        .into()
}

/// Play and Shuffle, and the action strip for the whole set.
pub fn play_buttons<'a>(
    tracks: &'a [TrackMeta],
    anchor: String,
    source: PlaySource,
) -> Element<'a, Message> {
    play_buttons_with(tracks, anchor, source, Vec::new())
}

/// Play and Shuffle, then `extras` (Start radio), then the "…" menu last,
/// as Apple Music orders an album's actions. The row wraps when narrow.
pub fn play_buttons_with<'a>(
    tracks: &'a [TrackMeta],
    anchor: String,
    source: PlaySource,
    extras: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let enabled = !tracks.is_empty();
    // Icons don't fade with a disabled button's label: quiet them here.
    let tone = |on: Tone| if enabled { on } else { Tone::Secondary };
    row![
        button(
            row![icon(Icon::Play, 14.0, tone(Tone::OnAccent)), text("Play")]
                .spacing(6)
                .align_y(Center)
        )
        .style(theme::filled)
        .padding([8, 18])
        .on_press_maybe(enabled.then(|| Message::Play(PlayRequest::from(tracks, 0, source)))),
        button(
            row![
                icon(Icon::Shuffle, 14.0, tone(Tone::Accent)),
                text("Shuffle")
            ]
            .spacing(6)
            .align_y(Center)
        )
        .style(theme::tonal)
        .padding([8, 18])
        .on_press_maybe(
            enabled.then(|| Message::Play(PlayRequest::shuffled(tracks, PlaySource::Shuffle)))
        ),
    ]
    .extend(extras)
    .push(
        button(icon(Icon::More, 18.0, tone(Tone::Accent)))
            .style(theme::tonal)
            .padding([8, 10])
            .on_press_maybe(enabled.then(|| {
                Message::Row(RowAction::Open {
                    anchor,
                    tracks: tracks.to_vec(),
                })
            })),
    )
    .spacing(10)
    .align_y(Center)
    .wrap()
    .vertical_spacing(10)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_list_names_the_artist_only_where_it_differs() {
        let track = |artist: &str| {
            let mut meta = TrackMeta::from_item(
                &serde_json::from_value(serde_json::json!({
                    "Id": "t1", "Name": "Harbour Lights", "Type": "Audio",
                    "Album": "Low Tide Radio", "AlbumArtist": "The Quiet Engines",
                }))
                .unwrap(),
            );
            meta.artist = artist.into();
            meta
        };
        let album = ListStyle {
            numbered: true,
            show_album: false,
        };
        assert_eq!(subtitle(&track("The Quiet Engines"), album), None);
        assert_eq!(subtitle(&track("the quiet engines "), album), None);
        assert_eq!(
            subtitle(&track("The Quiet Engines, Mara Vey"), album).as_deref(),
            Some("The Quiet Engines, Mara Vey")
        );
        let unknown = TrackMeta {
            album_artist: None,
            ..track("Mara Vey")
        };
        assert_eq!(subtitle(&unknown, album).as_deref(), Some("Mara Vey"));
        // Anywhere else, the artist is always there.
        let songs = ListStyle {
            numbered: false,
            show_album: true,
        };
        assert_eq!(
            subtitle(&track("The Quiet Engines"), songs).as_deref(),
            Some("The Quiet Engines · Low Tide Radio")
        );
    }
}
