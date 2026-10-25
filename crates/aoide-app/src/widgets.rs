//! The pieces every screen draws with: artwork, tiles, shelves, track rows
//! and the action strip under a row.

use aoide_store::PlaySource;
use iced::widget::{
    Column, Row, button, column, container, grid, image, mouse_area, row, scrollable, space, text,
    text_input, toggler,
};
use iced::{Center, ContentFit, Element, Fill, Font, Length, Shrink, font};

use crate::actions::{self, Collection, Menu, PlayRequest, RowAction};
use crate::app::Message;
use crate::artwork::{ArtKey, Artwork};
use crate::failure::Failure;
use crate::icons::{Icon, Tone, icon};
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::screens::{Route, View};
use crate::theme;

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

/// What a tile's `…` (or a right click on it) opens: the album's or
/// artist's actions.
fn tile_menu(tile: &Tile, route: &Route) -> Option<Message> {
    let collection = match route {
        Route::Album { id, .. } => Collection::Album(id.clone()),
        Route::Artist { id, .. } => Collection::Artist(id.clone()),
        Route::Playlist { id } => Collection::Playlist(id.clone()),
        _ => return None,
    };
    Some(Message::Row(RowAction::OpenCollection {
        anchor: format!("tile:{}", tile.id),
        collection,
        title: tile.title.clone(),
    }))
}

/// A tile's title and subtitle, with its `…` beside them.
fn tile_caption<'a>(tile: &'a Tile, menu: Option<Message>) -> Element<'a, Message> {
    let words = column![
        text(tile.title.as_str())
            .size(14)
            .font(SEMIBOLD)
            .wrapping(text::Wrapping::None),
        secondary(tile.subtitle.as_str())
            .size(13)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(2)
    .width(Fill)
    .clip(true);
    match menu {
        Some(menu) => row![
            words,
            button(icon(Icon::More, 16.0, Tone::Secondary))
                .style(theme::row)
                .padding(4)
                .on_press(menu),
        ]
        .align_y(Center)
        .into(),
        None => words.into(),
    }
}

/// A tile that opens what it shows; a right click opens its actions.
fn tile_button<'a>(
    body: impl Into<Element<'a, Message>>,
    route: Route,
    menu: Option<Message>,
) -> Element<'a, Message> {
    let button = button(body)
        .padding(4)
        .style(theme::row)
        .on_press(Message::Open(route));
    match menu {
        Some(menu) => mouse_area(button).on_right_press(menu).into(),
        None => button.into(),
    }
}

/// An album or artist tile that opens it.
fn tile_body<'a>(
    tile: &'a Tile,
    artwork: &'a Artwork,
    art_size: Length,
    menu: Option<Message>,
) -> Column<'a, Message> {
    column![
        art(tile.art.as_ref(), artwork, art_size, 8.0),
        tile_caption(tile, menu),
    ]
    .spacing(4)
    .clip(true)
}

/// A grid of tiles that fills the width, a few columns or many.
pub fn tile_grid<'a>(
    tiles: &'a [Tile],
    artwork: &'a Artwork,
    open: impl Fn(&Tile) -> Route,
) -> Element<'a, Message> {
    let cells = tiles.iter().map(|tile| {
        let route = open(tile);
        let menu = tile_menu(tile, &route);
        let body = column![
            container(art(tile.art.as_ref(), artwork, Fill, 8.0)).height(Fill),
            tile_caption(tile, menu.clone()),
        ]
        .spacing(4)
        .clip(true);
        tile_button(body, route, menu)
    });
    grid(cells)
        .fluid(210)
        .spacing(16)
        .height(grid::aspect_ratio(100, 128))
        .into()
}

/// A shelf: one row of tiles, scrolled sideways.
pub fn shelf<'a>(
    title: &'a str,
    tiles: &'a [Tile],
    artwork: &'a Artwork,
    open: impl Fn(&Tile) -> Route,
) -> Element<'a, Message> {
    let row = row(tiles.iter().map(|tile| {
        let route = open(tile);
        let menu = tile_menu(tile, &route);
        tile_button(
            tile_body(tile, artwork, Length::Fixed(160.0), menu.clone()).width(160),
            route,
            menu,
        )
    }))
    .spacing(12)
    .padding(iced::Padding::ZERO.bottom(14));
    column![
        text(title).size(20).font(BOLD),
        scrollable(row).horizontal().width(Fill),
    ]
    .spacing(10)
    .into()
}

/// A screen's title, with a back button when there is somewhere to go back to.
pub fn title_bar<'a>(title: &'a str, back: bool) -> Element<'a, Message> {
    let mut bar = row![].spacing(8).align_y(Center);
    if back {
        bar = bar.push(
            button(icon(Icon::Back, 20.0, Tone::Accent))
                .style(theme::row)
                .padding(4)
                .on_press(Message::Back),
        );
    }
    bar.push(text(title).size(28).font(BOLD)).into()
}

/// The scrollable every page is drawn in, put back at its top when another
/// page opens.
pub const PAGE: &str = "page-scroll";

/// A page's whole body: scrolled, padded.
pub fn page<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(container(content).padding([24, 28]).width(Fill))
        .id(PAGE)
        .height(Fill)
        .into()
}

pub fn loading<'a>() -> Element<'a, Message> {
    container(secondary("Loading…")).padding(20).into()
}

/// A failure whose words are all there is: the local store's, the app's own.
pub fn plain_failure<'a>(error: &'a str) -> Element<'a, Message> {
    container(text(error))
        .padding(12)
        .style(theme::error_box)
        .into()
}

/// Something that failed, in words: Retry runs it again (when there is
/// something to run), and Details shows the technical text under it.
pub fn failure<'a>(
    failure: &'a Failure,
    retry: Option<Message>,
    details_open: bool,
) -> Element<'a, Message> {
    let has_actions = retry.is_some() || failure.details.is_some();
    let mut actions = row![].spacing(8).align_y(Center);
    if let Some(retry) = retry {
        actions = actions.push(
            button(text("Retry").size(13))
                .style(theme::tonal)
                .padding([4, 12])
                .on_press(retry),
        );
    }
    if failure.details.is_some() {
        actions = actions.push(
            button(
                row![
                    text("Details").size(13),
                    icon(
                        if details_open { Icon::Up } else { Icon::Down },
                        14.0,
                        Tone::Secondary
                    ),
                ]
                .spacing(4)
                .align_y(Center),
            )
            .style(theme::row)
            .padding([4, 8])
            .on_press(Message::FailureDetails(!details_open)),
        );
    }
    let mut body = column![text(failure.summary.as_str())].spacing(10);
    if has_actions {
        body = body.push(actions);
    }
    if let (true, Some(details)) = (details_open, &failure.details) {
        body = body.push(secondary(details.as_str()).size(12));
    }
    container(body)
        .padding(12)
        .width(Fill)
        .style(theme::error_box)
        .into()
}

pub fn empty<'a>(message: &'a str) -> Element<'a, Message> {
    container(secondary(message)).padding(20).into()
}

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
    let lead: Element<'a, Message> = if style.numbered {
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
    };
    let subtitle = subtitle(track, style);
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
    let mut lines = column![
        text(track.title.as_str())
            .size(14)
            .wrapping(text::Wrapping::None)
            .style(move |t| text::Style {
                color: playing.then(|| theme::accent(t)),
            }),
    ];
    if let Some(subtitle) = subtitle {
        lines = lines.push(secondary(subtitle).size(12).wrapping(text::Wrapping::None));
    }
    // Every row as tall as one with two lines, the title centred when it is
    // alone: an album's rows stay even where only some name an artist.
    let lines = container(lines.spacing(2).clip(true))
        .width(Fill)
        .height(Length::Fixed(TWO_LINES))
        .align_y(Center)
        .clip(true);
    let body = row![lead, lines].spacing(12).align_y(Center);
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

/// The action strip under a row: Play Next, Play Last, Add to Playlist, the
/// taste flags, Favourite, Go to.
pub fn menu_strip<'a>(menu: &'a Menu, view: &View<'a>) -> Element<'a, Message> {
    let action = |label: &'a str, action: RowAction| {
        button(text(label).size(13))
            .style(theme::tonal)
            .padding([6, 10])
            .on_press(Message::Row(action))
    };
    let check = |label: &'static str, on: bool, act: RowAction| {
        let mut body = row![].spacing(4).align_y(Center);
        if on {
            body = body.push(icon(Icon::Check, 13.0, Tone::Accent));
        }
        button(body.push(text(label).size(13)))
            .style(theme::tonal)
            .padding([6, 10])
            .on_press(Message::Row(act))
    };
    // A collection's actions wait for its songs.
    let ready = !menu.loading && !menu.tracks.is_empty();
    let plain = action;
    let action = move |label: &'a str, act: RowAction| {
        let button = plain(label, act);
        if ready || menu.collection.is_none() {
            button
        } else {
            button.on_press_maybe(None)
        }
    };
    let mut actions: Vec<Element<'a, Message>> = Vec::new();
    if menu.collection.is_some() {
        actions.push(action("Play", RowAction::Play).into());
        actions.push(action("Shuffle", RowAction::Shuffle).into());
    }
    actions.extend([
        action("Play next", RowAction::PlayNext).into(),
        action("Play last", RowAction::PlayLast).into(),
        action("Add to playlist…", RowAction::ShowPlaylists).into(),
    ]);
    if menu.single().is_some() || menu.collection.is_some() {
        actions.push(action("Start radio", RowAction::StartRadio).into());
    }
    if let Some(track) = menu.single() {
        let flags = menu.flags.unwrap_or_default();
        let favourite = view
            .favourites
            .get(&track.id)
            .copied()
            .unwrap_or(track.favourite);
        actions.push(
            check(
                "Not interested",
                flags.not_interested,
                RowAction::NotInterested,
            )
            .into(),
        );
        actions.push(check("Don't count plays", flags.dont_count, RowAction::DontCount).into());
        actions.push(
            action(
                if favourite {
                    "Unfavourite"
                } else {
                    "Favourite"
                },
                RowAction::Favourite,
            )
            .into(),
        );
        if track.album_id.is_some() {
            actions.push(action("Go to album", RowAction::GoToAlbum).into());
        }
        if track.artist_id.is_some() {
            actions.push(action("Go to artist", RowAction::GoToArtist).into());
        }
        actions.push(action("Get info", RowAction::GetInfo).into());
    }
    actions.push(
        button(icon(Icon::Close, 14.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(Message::Row(RowAction::Close))
            .into(),
    );
    let actions = row(actions).spacing(6).wrap().vertical_spacing(6);

    let mut body = column![].spacing(10);
    if let Some((_, title)) = &menu.collection {
        body = body.push(text(title.as_str()).size(16).font(SEMIBOLD));
    }
    body = body.push(actions);
    if menu.loading {
        body = body.push(secondary("Fetching the songs…").size(13));
    }
    if let Some(error) = &menu.error {
        body = body.push(plain_failure(error));
    }
    if menu.adding {
        let mut lists = column![].spacing(2);
        if view.playlists.len() > actions::PLAYLIST_SEARCH_AFTER {
            lists = lists.push(
                text_input("Find a playlist", &menu.filter)
                    .on_input(|s| Message::Row(RowAction::Filter(s)))
                    .padding(6)
                    .size(13)
                    .style(theme::field),
            );
        }
        let heading = |label: &'static str| secondary(label).size(12).font(SEMIBOLD);
        lists = lists.push(heading("Aoide playlists"));
        for playlist in actions::matching(view.playlists, |p| p.name.as_str(), &menu.filter) {
            lists = lists.push(
                button(text(playlist.name.as_str()).size(13))
                    .width(Fill)
                    .style(theme::row)
                    .padding([4, 8])
                    .on_press(Message::Row(RowAction::AddTo(playlist.id.clone()))),
            );
        }
        lists = lists.push(heading("Jellyfin playlists"));
        match &menu.jellyfin {
            None => {}
            Some(Err(error)) => lists = lists.push(secondary(error.as_str()).size(12)),
            Some(Ok(jellyfin)) => {
                for (id, name) in actions::matching(jellyfin, |p| p.1.as_str(), &menu.filter) {
                    lists = lists.push(
                        button(text(name.as_str()).size(13))
                            .width(Fill)
                            .style(theme::row)
                            .padding([4, 8])
                            .on_press(Message::Row(RowAction::AddToJellyfin(id.clone()))),
                    );
                }
            }
        }
        let create = row![
            text_input("New playlist…", &menu.new_name)
                .on_input(|s| Message::Row(RowAction::NewName(s)))
                .on_submit(Message::Row(RowAction::CreatePlaylist))
                .padding(6)
                .size(13)
                .style(theme::field),
            button(text("Create").size(13))
                .style(theme::filled)
                .padding([6, 10])
                .on_press_maybe(
                    (!menu.new_name.trim().is_empty())
                        .then_some(Message::Row(RowAction::CreatePlaylist))
                ),
        ]
        .spacing(6)
        .align_y(Center);
        body = body.push(lists).push(create);
    }
    container(body)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
}

/// Play and Shuffle, and the action strip for the whole set.
pub fn play_buttons<'a>(
    tracks: &'a [TrackMeta],
    anchor: String,
    source: PlaySource,
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
        button(icon(Icon::More, 18.0, tone(Tone::Accent)))
            .style(theme::tonal)
            .padding([8, 10])
            .on_press_maybe(enabled.then(|| {
                Message::Row(RowAction::Open {
                    anchor,
                    tracks: tracks.to_vec(),
                })
            })),
    ]
    .spacing(10)
    .width(Shrink)
    .into()
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
