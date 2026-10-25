//! Importing a playlist from elsewhere: a pasted Spotify link (its public
//! embed page, the first 100 tracks, no account) or a CSV export (Exportify's
//! columns and the other tools' names for them). The tracks are matched
//! against the library by the sidecar's `POST /aoide/match`, or by the shared
//! rules over Jellyfin search where there is no sidecar; then what was found
//! is saved as an Aoide playlist, and what was not can be copied as text.

use aoide_jellyfin::{Item, JellyfinClient, ListQuery};
use aoide_sidecar::{
    ImportMatch, ImportResult, ImportedPlaylist, ImportedTrack, JellyfinSearch, LibraryTrack,
    MatchedBy, SidecarClient, fetch_spotify_playlist, fetch_spotify_playlist_from, match_playlist,
    parse_playlist_csv,
};
use iced::futures::{SinkExt, Stream};
use iced::widget::{Column, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, PageMsg, Route, View};
use crate::app::Message as App;
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

/// Jellyfin's song search, for the matching rules when there is no sidecar.
#[derive(Clone)]
pub struct LibrarySearch {
    pub client: JellyfinClient,
    pub library: Option<String>,
}

/// A song as the matcher compares it: its artists with the album artist
/// added, since either may be the name the other service credits.
pub fn library_track(item: &Item) -> LibraryTrack {
    let mut artists = item.artists.clone();
    if let Some(album_artist) = item.album_artist.as_ref().filter(|a| !a.is_empty())
        && !artists.iter().any(|a| a == album_artist)
    {
        artists.push(album_artist.clone());
    }
    LibraryTrack {
        jellyfin_id: item.id.clone(),
        title: item.name.clone(),
        artists,
        album: item.album.clone(),
        duration_ms: item
            .duration()
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        isrc: None,
    }
}

impl JellyfinSearch for LibrarySearch {
    fn search_songs(
        &self,
        term: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<LibraryTrack>, String>> + Send {
        let client = self.client.clone();
        let query = ListQuery {
            parent_id: self.library.clone(),
            search_term: Some(term.to_owned()),
            limit: Some(u32::try_from(limit).unwrap_or(u32::MAX)),
            ..ListQuery::default()
        };
        async move {
            client
                .songs(&query)
                .await
                .map(|page| page.items.iter().map(library_track).collect())
                .map_err(|e| crate::session::describe(&e))
        }
    }
}

/// Where the Spotify page comes from. A development build can point it at a
/// local stand-in, so the screen can be tried without the internet.
async fn read_spotify(link: String) -> Result<ImportedPlaylist, String> {
    let origin = cfg!(debug_assertions)
        .then(|| std::env::var("AOIDE_DEV_SPOTIFY_ORIGIN").ok())
        .flatten();
    match origin {
        Some(origin) => fetch_spotify_playlist_from(&origin, &link).await,
        None => fetch_spotify_playlist(&link).await,
    }
    .map_err(|e| {
        let mut said = e.to_string();
        if let Some(first) = said.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{said}.")
    })
}

/// The missing tracks as lines to paste somewhere: "Title — Artist".
pub fn missing_text(result: &ImportResult) -> String {
    result
        .missing()
        .iter()
        .map(|track| line(track))
        .collect::<Vec<_>>()
        .join("\n")
}

fn line(track: &ImportedTrack) -> String {
    if track.artists.is_empty() {
        track.title.clone()
    } else {
        format!("{} \u{2014} {}", track.title, track.artists.join(", "))
    }
}

/// "Found 35 of 40 songs."
pub fn summary(result: &ImportResult) -> String {
    let found = result.found_ids().len();
    let total = result.matches.len();
    let by = match result.by {
        MatchedBy::Sidecar => "by your server",
        MatchedBy::Library => "by searching your library",
    };
    format!("Found {found} of {total} songs, {by}.")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Waiting for a link or a file.
    Start,
    Reading,
    Matching {
        done: usize,
        total: usize,
    },
    Matched(Box<(ImportedPlaylist, ImportResult)>),
    Saving,
}

#[derive(Debug)]
pub struct State {
    link: String,
    name: String,
    step: Step,
    error: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Link(String),
    ReadLink,
    ChooseCsv,
    /// A file picked and read: its name and text. `None`: cancelled.
    Csv(Option<Result<(String, String), String>>),
    Read(Result<ImportedPlaylist, String>),
    Progress(usize, usize),
    Matched(Box<(ImportedPlaylist, ImportResult)>),
    Name(String),
    Save,
    Saved(Result<String, String>),
    CopyMissing,
    StartOver,
}

pub fn open(_ctx: &Ctx<'_>) -> (State, Task<App>) {
    (
        State {
            link: String::new(),
            name: String::new(),
            step: Step::Start,
            error: None,
            note: None,
        },
        Task::none(),
    )
}

/// Matching, as a stream: progress, then the result.
fn matching(
    playlist: ImportedPlaylist,
    sidecar: Option<SidecarClient>,
    library: LibrarySearch,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, async move |mut output| {
        let mut progress = output.clone();
        let result = match_playlist(
            sidecar.as_ref(),
            &library,
            &playlist.tracks,
            |done, total| {
                let _ = progress.try_send(Message::Progress(done, total));
            },
        )
        .await;
        let _ = output
            .send(Message::Matched(Box::new((playlist, result))))
            .await;
    })
}

/// Picks a CSV file and reads it as text.
async fn pick_csv() -> Option<Result<(String, String), String>> {
    let file = rfd::AsyncFileDialog::new()
        .set_title("Choose a playlist export")
        .add_filter("CSV", &["csv", "txt"])
        .pick_file()
        .await?;
    let path = file.path().to_owned();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(
        tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
            .await
            .map_err(|e| e.to_string())
            .and_then(|read| read.map_err(|e| e.to_string()))
            .map(|text| (name, text)),
    )
}

impl crate::app::App {
    /// Matches an imported playlist for the import page with `key`: the
    /// sidecar first, the library's search where there is none.
    pub(crate) fn import_match(&mut self, key: u64, playlist: ImportedPlaylist) -> Task<App> {
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let library = LibrarySearch {
            client: signed.connection.client.clone(),
            library: signed.library.clone(),
        };
        let sidecar = signed.sidecar.clone();
        Task::run(matching(playlist, sidecar, library), move |m| {
            App::Page(key, PageMsg::Import(m))
        })
    }
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        let key = ctx.key;
        let wrap = move |m: Message| App::Page(key, PageMsg::Import(m));
        match message {
            Message::Link(link) => {
                self.link = link;
                Task::none()
            }
            Message::ReadLink => {
                let link = self.link.trim().to_owned();
                if link.is_empty() {
                    return Task::none();
                }
                self.error = None;
                self.step = Step::Reading;
                Task::perform(read_spotify(link), move |r| wrap(Message::Read(r)))
            }
            Message::ChooseCsv => {
                self.error = None;
                Task::perform(pick_csv(), move |file| wrap(Message::Csv(file)))
            }
            Message::Csv(None) => Task::none(),
            Message::Csv(Some(read)) => {
                let parsed = read
                    .map_err(|e| format!("That file couldn't be read: {e}"))
                    .and_then(|(name, text)| {
                        parse_playlist_csv(&text, &name).map_err(|e| {
                            let said = e.to_string();
                            format!("{}{}.", said[..1].to_uppercase(), &said[1..])
                        })
                    });
                Task::done(wrap(Message::Read(parsed)))
            }
            Message::Read(Err(error)) => {
                self.step = Step::Start;
                self.error = Some(error);
                Task::none()
            }
            Message::Read(Ok(playlist)) => {
                if playlist.tracks.is_empty() {
                    self.step = Step::Start;
                    self.error = Some("That playlist has no songs in it.".to_owned());
                    return Task::none();
                }
                self.name.clone_from(&playlist.name);
                self.step = Step::Matching {
                    done: 0,
                    total: playlist.tracks.len(),
                };
                // The app holds the sidecar client the matching asks first.
                Task::done(App::ImportMatch(key, Box::new(playlist)))
            }
            Message::Progress(done, total) => {
                if matches!(self.step, Step::Matching { .. }) {
                    self.step = Step::Matching { done, total };
                }
                Task::none()
            }
            Message::Matched(found) => {
                self.step = Step::Matched(found);
                Task::none()
            }
            Message::Name(name) => {
                self.name = name;
                Task::none()
            }
            Message::Save => {
                let Step::Matched(found) = &self.step else {
                    return Task::none();
                };
                let ids = found.1.found_ids();
                let name = self.name.trim().to_owned();
                if ids.is_empty() || name.is_empty() {
                    return Task::none();
                }
                self.step = Step::Saving;
                let client = ctx.client.clone();
                let store = ctx.store.clone();
                Task::perform(
                    async move {
                        let items = client
                            .items_by_ids(&ids)
                            .await
                            .map_err(|e| crate::session::describe(&e))?;
                        let tracks: Vec<TrackMeta> =
                            items.iter().map(TrackMeta::from_item).collect();
                        let now = aoide_store::now_millis();
                        store
                            .call(move |w| {
                                let list = w.store.create_playlist(&name, None, None, now)?;
                                let rows: Vec<_> =
                                    tracks.iter().map(|t| t.track_ref(now)).collect();
                                w.store.cache_tracks(&rows)?;
                                let links: Vec<_> = tracks.iter().map(TrackMeta::link).collect();
                                w.store.add_tracks(&links, &list.id, now)?;
                                Ok::<_, aoide_store::StoreError>(list.id)
                            })
                            .await
                            .ok_or_else(|| "the library is closing".to_owned())?
                            .map_err(|e| e.to_string())
                    },
                    move |r| wrap(Message::Saved(r)),
                )
            }
            Message::Saved(Ok(id)) => {
                // Back from the new playlist lands on a fresh import.
                self.step = Step::Start;
                self.link.clear();
                self.note = None;
                Task::done(App::PlaylistsChanged)
                    .chain(Task::done(App::Open(Route::Playlist { id })))
            }
            Message::Saved(Err(error)) => {
                self.error = Some(format!("The playlist couldn't be saved: {error}"));
                Task::none()
            }
            Message::CopyMissing => {
                let Step::Matched(found) = &self.step else {
                    return Task::none();
                };
                self.note = Some("The missing songs are on the clipboard.".to_owned());
                iced::clipboard::write(missing_text(&found.1))
            }
            Message::StartOver => {
                self.step = Step::Start;
                self.note = None;
                self.error = None;
                Task::none()
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Import(m));
        let mut body = column![
            widgets::title_bar("Import a Playlist", view.can_go_back),
            widgets::secondary(
                "Bring a playlist from Spotify or another service. Aoide looks for each song \
                 in your library and saves the ones it finds as an Aoide playlist."
            )
            .size(13),
        ]
        .spacing(16);
        match &self.step {
            Step::Start => {
                let ready = !self.link.trim().is_empty();
                body = body
                    .push(
                        row![
                            text_input("Paste a Spotify playlist link", &self.link)
                                .on_input(move |s| message(Message::Link(s)))
                                .on_submit_maybe(ready.then(|| message(Message::ReadLink)))
                                .padding(8)
                                .style(theme::field),
                            button(text("Read"))
                                .style(theme::filled)
                                .padding([8, 14])
                                .on_press_maybe(ready.then(|| message(Message::ReadLink))),
                        ]
                        .spacing(10)
                        .align_y(Center),
                    )
                    .push(
                        row![
                            widgets::secondary("Or a CSV export (Exportify and similar):").size(13),
                            button(text("Choose a CSV File…").size(13))
                                .style(theme::tonal)
                                .padding([6, 12])
                                .on_press(message(Message::ChooseCsv)),
                        ]
                        .spacing(10)
                        .align_y(Center),
                    )
                    .push(
                        widgets::secondary(
                            "A Spotify link gives the first 100 songs; export the playlist to \
                             CSV for all of it. Only the link's public page is read: no account.",
                        )
                        .size(12),
                    );
            }
            Step::Reading => body = body.push(widgets::secondary("Reading the playlist…")),
            Step::Matching { done, total } => {
                body = body.push(widgets::secondary(format!(
                    "Looking for the songs in your library: {done} of {total}…"
                )));
            }
            Step::Saving => body = body.push(widgets::secondary("Saving…")),
            Step::Matched(found) => body = body.push(self.matched(&found.0, &found.1, view)),
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::failure(error));
        }
        if let Some(note) = &self.note {
            body = body.push(widgets::secondary(note.as_str()).size(13));
        }
        widgets::page(body.max_width(860))
    }

    fn matched<'a>(
        &'a self,
        playlist: &'a ImportedPlaylist,
        result: &'a ImportResult,
        view: &View<'a>,
    ) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Import(m));
        let found = result.found_ids().len();
        let ready = found > 0 && !self.name.trim().is_empty();
        let mut column = column![text(summary(result)).size(16)].spacing(14);
        if playlist.is_truncated {
            column = column.push(
                widgets::secondary(
                    "Spotify only shares the first 100 songs through a link. Export the \
                     playlist to CSV for all of it.",
                )
                .size(13),
            );
        }
        column = column.push(
            row![
                text_input("Playlist name", &self.name)
                    .on_input(move |s| message(Message::Name(s)))
                    .padding(8)
                    .style(theme::field),
                button(text(match found {
                    1 => "Save 1 Song".to_owned(),
                    n => format!("Save {n} Songs"),
                }))
                .style(theme::filled)
                .padding([8, 14])
                .on_press_maybe(ready.then(|| message(Message::Save))),
            ]
            .spacing(10)
            .align_y(Center),
        );
        let missing = result.missing();
        let mut tools = row![
            button(text("Start Over").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press(message(Message::StartOver)),
        ]
        .spacing(8);
        if !missing.is_empty() {
            tools = tools.push(
                button(text("Copy Missing Songs").size(13))
                    .style(theme::tonal)
                    .padding([6, 12])
                    .on_press(message(Message::CopyMissing)),
            );
        }
        column = column.push(tools);
        let rows = |matches: Vec<&'a ImportMatch>, found: bool| {
            let mut list = Column::new().spacing(4);
            for m in matches {
                let mut line = column![text(line(&m.track)).size(14)];
                if found && let Some(chosen) = &m.found {
                    let artist = chosen
                        .artists
                        .first()
                        .map(String::as_str)
                        .unwrap_or_default();
                    line = line.push(
                        widgets::secondary(format!(
                            "In your library: {} \u{2014} {artist}",
                            chosen.title.as_deref().unwrap_or("this song")
                        ))
                        .size(12),
                    );
                }
                list = list.push(container(line).padding([4, 8]));
            }
            container(list).padding(10).width(Fill).style(theme::menu)
        };
        let (have, lack): (Vec<&ImportMatch>, Vec<&ImportMatch>) =
            result.matches.iter().partition(|m| m.found.is_some());
        if !lack.is_empty() {
            column = column
                .push(
                    text(format!("Not in Your Library ({})", lack.len()))
                        .size(17)
                        .font(widgets::BOLD),
                )
                .push(rows(lack, false));
        }
        if !have.is_empty() {
            column = column
                .push(
                    text(format!("Found ({})", have.len()))
                        .size(17)
                        .font(widgets::BOLD),
                )
                .push(rows(have, true));
        }
        column.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use aoide_sidecar::MatchedTrack;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn track(title: &str, artist: &str) -> ImportedTrack {
        ImportedTrack::new(title, &[artist], Some(200_000))
    }

    #[test]
    fn the_missing_songs_copy_as_lines() {
        let result = ImportResult {
            matches: vec![
                ImportMatch {
                    track: track("Lantern", "Mara Vey"),
                    found: Some(MatchedTrack {
                        jellyfin_id: "t1".into(),
                        title: Some("Lantern".into()),
                        artists: vec!["Mara Vey".into()],
                        album: None,
                        duration_ms: None,
                        score: 1.0,
                    }),
                },
                ImportMatch {
                    track: track("Undertow", "Odd Hours"),
                    found: None,
                },
                ImportMatch {
                    track: ImportedTrack::new("Untitled", &[], None),
                    found: None,
                },
            ],
            by: MatchedBy::Library,
        };
        assert_eq!(
            missing_text(&result),
            "Undertow \u{2014} Odd Hours\nUntitled"
        );
        assert_eq!(
            summary(&result),
            "Found 1 of 3 songs, by searching your library."
        );
    }

    #[test]
    fn a_library_song_offers_its_album_artist_too() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "Id": "t1", "Name": "Lantern", "Type": "Audio", "Artists": ["Mara Vey feat. Robin"],
            "AlbumArtist": "Mara Vey", "RunTimeTicks": 2_000_000_000_i64
        }))
        .unwrap();
        let song = library_track(&item);
        assert_eq!(song.artists, ["Mara Vey feat. Robin", "Mara Vey"]);
        assert_eq!(song.duration_ms, Some(200_000));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_a_sidecar_the_library_is_searched_and_matched_by_the_rules() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("searchTerm", "Lantern"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "t1", "Name": "Lantern", "Type": "Audio",
                           "Artists": ["Mara Vey"], "RunTimeTicks": 2_000_000_000_i64}],
                "TotalRecordCount": 1
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [], "TotalRecordCount": 0
            })))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let library = LibrarySearch {
            client,
            library: Some("lib1".into()),
        };
        let tracks = [track("Lantern", "Mara Vey"), track("Undertow", "Odd Hours")];
        let mut seen = Vec::new();
        let result = match_playlist(None, &library, &tracks, |done, total| {
            seen.push((done, total))
        })
        .await;
        assert_eq!(result.found_ids(), ["t1"]);
        assert_eq!(result.by, MatchedBy::Library);
        assert_eq!(seen.last(), Some(&(2, 2)));
    }
}
