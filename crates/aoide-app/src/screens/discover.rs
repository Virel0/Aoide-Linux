//! Discover: this week's 30 songs from the sidecar's `GET /aoide/discover`,
//! songs in the library not yet heard all the way through, chosen by what is
//! listened to. The list holds still for the ISO week (UTC), so it is fetched
//! when Home opens and again only once the week has turned. A 404 is an older
//! sidecar: the tile stays hidden for the session. The iOS app's
//! `DiscoverStore` and `DiscoverView`.

use aoide_jellyfin::JellyfinClient;
use aoide_sidecar::{DiscoverResponse, SidecarClient, SidecarError, iso_week};
use aoide_store::PlaySource;
use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, Load, PageMsg, Route, View};
use crate::app::{App, Message as AppMessage};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

/// What the app holds of this week's list.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Week {
    /// Not asked yet, or the last ask failed for a reason worth retrying.
    #[default]
    Unknown,
    /// The server has no `/aoide/discover`: hide the tile.
    Unavailable,
    Loaded {
        week: String,
        tracks: Vec<TrackMeta>,
    },
}

impl Week {
    /// Whether to ask the server now: never after a 404, and for a loaded list
    /// only once the UTC week has moved on.
    pub fn needs_fetch(&self, now: aoide_store::Millis) -> bool {
        match self {
            Week::Unavailable => false,
            Week::Loaded { week, .. } => *week != iso_week(now),
            Week::Unknown => true,
        }
    }

    /// The count the Home tile shows; `None` hides it.
    pub fn count(&self) -> Option<usize> {
        match self {
            Week::Loaded { tracks, .. } if !tracks.is_empty() => Some(tracks.len()),
            _ => None,
        }
    }
}

/// The app's side of Discover: the week, and whether a fetch is in flight.
#[derive(Debug, Default)]
pub struct Discover {
    pub week: Week,
    loading: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// A Home page opened: fetch if due, and tell it the count.
    Home(u64),
    Fetched(Result<Week, String>),
    /// The Discover page with this key wants the list.
    Show(u64),
}

/// The week's list with its songs, in the server's order.
pub async fn fetch(client: JellyfinClient, sidecar: SidecarClient) -> Result<Week, String> {
    let response = match sidecar.discover(DiscoverResponse::DEFAULT_LIMIT).await {
        Ok(response) => response,
        Err(SidecarError::Absent { .. }) => return Ok(Week::Unavailable),
        Err(error) => return Err(crate::failure::sidecar(&error).to_string()),
    };
    let ids: Vec<String> = response.candidates.iter().map(|c| c.id.clone()).collect();
    let items = client
        .items_by_ids(&ids)
        .await
        .map_err(|e| crate::failure::describe(&e).to_string())?;
    Ok(Week::Loaded {
        week: response.week,
        tracks: items.iter().map(TrackMeta::from_item).collect(),
    })
}

impl App {
    pub(crate) fn discover(&mut self, message: Msg) -> Task<AppMessage> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::Home(key) => {
                let count = signed.discover.week.count();
                let tell = Task::done(AppMessage::Page(
                    key,
                    PageMsg::Home(super::home::Message::Discover(count)),
                ));
                let now = aoide_store::now_millis();
                let Some(sidecar) = signed.sidecar.clone() else {
                    return tell;
                };
                if signed.discover.loading || !signed.discover.week.needs_fetch(now) {
                    return tell;
                }
                signed.discover.loading = true;
                let client = signed.connection.client.clone();
                Task::batch([
                    tell,
                    Task::perform(fetch(client, sidecar), |r| {
                        AppMessage::Discover(Msg::Fetched(r))
                    }),
                ])
            }
            Msg::Fetched(result) => {
                signed.discover.loading = false;
                match result {
                    Ok(week) => signed.discover.week = week,
                    // Asked again the next time Home opens.
                    Err(_) => return Task::none(),
                }
                let count = signed.discover.week.count();
                let homes: Vec<u64> = signed
                    .nav
                    .as_mut()
                    .map(|nav| {
                        nav.iter_mut()
                            .filter(|e| matches!(e.page, super::Page::Home(_)))
                            .map(|e| e.key)
                            .collect()
                    })
                    .unwrap_or_default();
                Task::batch(homes.into_iter().map(|key| {
                    Task::done(AppMessage::Page(
                        key,
                        PageMsg::Home(super::home::Message::Discover(count)),
                    ))
                }))
            }
            Msg::Show(key) => {
                let tracks = match &signed.discover.week {
                    Week::Loaded { tracks, .. } => Ok(tracks.clone()),
                    Week::Unavailable => Err("This server's Aoide sidecar has no Discover.".into()),
                    Week::Unknown => Err("This week's list hasn't arrived yet.".into()),
                };
                Task::done(AppMessage::Page(
                    key,
                    PageMsg::Discover(Message::Tracks(tracks)),
                ))
            }
        }
    }
}

// MARK: - The page

#[derive(Debug)]
pub struct State {
    pub tracks: Load<Vec<TrackMeta>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tracks(Result<Vec<TrackMeta>, String>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<AppMessage>) {
    (
        State {
            tracks: Load::Loading,
        },
        Task::done(AppMessage::Discover(Msg::Show(ctx.key))),
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<AppMessage> {
        match message {
            Message::Tracks(result) => self.tracks = Load::from(result),
        }
        Task::none()
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        self.tracks
            .ready()
            .map(|t| super::track_art(t))
            .unwrap_or_default()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);
        let mut body = column![
            widgets::title_bar("Discover", view.can_go_back),
            widgets::secondary(
                "Songs from your library you haven't heard all the way through, chosen by what \
                 you do listen to. The list holds still for the week and changes each Monday."
            )
            .size(14),
            widgets::play_buttons(tracks, format!("{}:discover", view.key), PlaySource::Smart),
        ]
        .spacing(16);
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{}:discover", view.key))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(tracks) if tracks.is_empty() => {
                widgets::empty("Nothing new this week: you've heard everything it would pick.")
            }
            Load::Ready(tracks) => widgets::track_list(
                tracks,
                view,
                widgets::ListStyle {
                    numbered: false,
                    show_album: true,
                },
                PlaySource::Smart,
            ),
        });
        widgets::page(body)
    }
}

/// The tile on Home: what Discover is and how many songs it holds.
pub fn tile<'a>(count: usize) -> Element<'a, AppMessage> {
    let words = column![
        text("Discover")
            .size(17)
            .font(widgets::BOLD)
            .style(|t: &iced::Theme| text::Style {
                color: Some(t.palette().text),
            }),
        widgets::secondary(format!(
            "{count} songs you own and haven't heard, new each Monday"
        ))
        .size(13),
    ]
    .spacing(2)
    .width(Fill);
    button(
        container(
            row![icon(Icon::Sparkle, 26.0, Tone::Accent), words]
                .spacing(14)
                .align_y(Center),
        )
        .padding(8),
    )
    .width(Fill)
    .style(theme::tonal)
    .padding(6)
    .on_press(AppMessage::Open(Route::Discover))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 2026-10-21, a Wednesday of week 43.
    const WEDNESDAY: i64 = 1_792_540_800_000;
    /// The Monday after, week 44.
    const NEXT_MONDAY: i64 = WEDNESDAY + 5 * 24 * 60 * 60 * 1000;

    fn loaded(week: &str, n: usize) -> Week {
        let track = TrackMeta::from_item(
            &serde_json::from_value(serde_json::json!({"Id": "t", "Name": "t", "Type": "Audio"}))
                .unwrap(),
        );
        Week::Loaded {
            week: week.into(),
            tracks: vec![track; n],
        }
    }

    #[test]
    fn the_list_is_fetched_once_a_week_and_never_after_a_404() {
        assert_eq!(iso_week(WEDNESDAY), "2026-W43");
        assert_eq!(iso_week(NEXT_MONDAY), "2026-W44");
        assert!(Week::Unknown.needs_fetch(WEDNESDAY));
        assert!(!loaded("2026-W43", 30).needs_fetch(WEDNESDAY));
        assert!(loaded("2026-W43", 30).needs_fetch(NEXT_MONDAY));
        assert!(!Week::Unavailable.needs_fetch(NEXT_MONDAY));
    }

    #[test]
    fn the_tile_shows_only_a_list_with_songs() {
        assert_eq!(loaded("2026-W43", 30).count(), Some(30));
        assert_eq!(loaded("2026-W43", 0).count(), None);
        assert_eq!(Week::Unavailable.count(), None);
        assert_eq!(Week::Unknown.count(), None);
    }

    fn client(server: &MockServer) -> JellyfinClient {
        JellyfinClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_weeks_songs_arrive_in_the_servers_order() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/aoide/discover"))
            .and(query_param("limit", "30"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "week": "2026-W43",
                "candidates": [
                    {"id": "b", "score": 0.9, "factors": {"taste": 0.6, "kinship": null}},
                    {"id": "a", "score": 0.8, "factors": {"taste": 0.4}}
                ],
                "profile": {"events": 12, "since": 0}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "a", "Name": "A", "Type": "Audio"}, {"Id": "b", "Name": "B", "Type": "Audio"}],
                "TotalRecordCount": 2
            })))
            .mount(&server)
            .await;
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let week = fetch(client(&server), sidecar).await.unwrap();
        let Week::Loaded { week, tracks } = week else {
            panic!("not loaded: {week:?}");
        };
        assert_eq!(week, "2026-W43");
        let ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_404_hides_the_tile() {
        let server = MockServer::start().await;
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        assert_eq!(
            fetch(client(&server), sidecar).await.unwrap(),
            Week::Unavailable
        );
    }
}
