//! Start Radio: a station seeded by a song, an album, an artist, a genre or a
//! playlist. The seed song (an album's, artist's or genre's first) leads, and
//! Melete's `/aoide/next` chooses what follows it, as Infinity does; the songs
//! marked Not Interested are left out. Played at once and remembered as a
//! station for the resume grid.

use aoide_melete::{Item, ListQuery, MeleteClient};
use aoide_sidecar::{NextMode, NextRequest, SidecarClient};
use aoide_store::{PlaySource, RecentContext, RecentContextKind, StationSeed};
use iced::widget::{button, row, text};
use iced::{Center, Element, Task};

use crate::actions::{Collection, Menu, PlayRequest};
use crate::app::{App, Message};
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::store_worker::StoreHandle;
use crate::theme;

/// How many songs a station starts with.
pub const LENGTH: u32 = 100;

/// What seeds a station.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MixSeed {
    Track(String),
    Album(String),
    Artist(String),
    /// A genre, by its name.
    Genre(String),
}

/// A station to start: what seeds it and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub seed: MixSeed,
    /// The seed's name: "Low Tide Radio" is made from it.
    pub name: String,
}

impl Station {
    pub fn new(seed: MixSeed, name: impl Into<String>) -> Self {
        Self {
            seed,
            name: name.into(),
        }
    }

    /// The station a menu stands for: its song, or its album, artist or
    /// playlist. A playlist is seeded by its first song.
    pub fn of_menu(menu: &Menu) -> Option<Self> {
        if let Some(track) = menu.single() {
            return Some(Self::new(MixSeed::Track(track.id.clone()), &track.title));
        }
        let (collection, title) = menu.collection.as_ref()?;
        let seed = match collection {
            Collection::Album(id) => MixSeed::Album(id.clone()),
            Collection::Artist(id) => MixSeed::Artist(id.clone()),
            Collection::Playlist(_) => MixSeed::Track(menu.tracks.first()?.id.clone()),
        };
        Some(Self::new(seed, title))
    }

    fn id(&self) -> &str {
        match &self.seed {
            MixSeed::Track(id) | MixSeed::Album(id) | MixSeed::Artist(id) | MixSeed::Genre(id) => {
                id
            }
        }
    }

    /// What the resume grid keeps: an album's or artist's station says what
    /// seeded it so it is seeded the same way again; the others are resumed
    /// from the same song or genre.
    pub fn context(&self) -> RecentContext {
        let name = format!("{} Radio", self.name);
        let now = aoide_store::now_millis();
        let mut context = match &self.seed {
            MixSeed::Album(id) => RecentContext::station(StationSeed::Album, id.clone(), name, now),
            MixSeed::Artist(id) => {
                RecentContext::station(StationSeed::Artist, id.clone(), name, now)
            }
            _ => RecentContext::new(RecentContextKind::Station, self.id(), name, now),
        };
        context.artwork_item_id = Some(self.id().to_owned());
        context
    }
}

/// The song a seed starts from: itself, or the first of its album, artist or
/// genre.
async fn seed_track(client: &MeleteClient, seed: &MixSeed) -> aoide_melete::Result<Option<Item>> {
    let first = |query: ListQuery| async move {
        let query = ListQuery {
            limit: Some(1),
            ..query
        };
        Ok(client.songs(&query).await?.items.into_iter().next())
    };
    match seed {
        MixSeed::Track(id) => Ok(client.items_by_ids(&[id]).await?.into_iter().next()),
        MixSeed::Album(id) => Ok(client.album_tracks(id).await?.into_iter().next()),
        MixSeed::Artist(id) => {
            first(ListQuery {
                artist_ids: vec![id.clone()],
                ..ListQuery::default()
            })
            .await
        }
        MixSeed::Genre(name) => {
            first(ListQuery {
                genre: Some(name.clone()),
                ..ListQuery::default()
            })
            .await
        }
    }
}

/// The station's songs: the seed song, then Melete's choices in its order,
/// Not Interested ones left out.
pub async fn tune(
    client: MeleteClient,
    sidecar: Option<SidecarClient>,
    store: StoreHandle,
    seed: MixSeed,
) -> Result<Vec<Item>, String> {
    let describe = |e: aoide_melete::Error| crate::failure::describe(&e).to_string();
    let Some(first) = seed_track(&client, &seed).await.map_err(describe)? else {
        return Ok(Vec::new());
    };
    let sidecar = sidecar.ok_or_else(|| "this server can't choose songs".to_owned())?;
    let request = NextRequest::new(&first.id, &[], &[], NextMode::Infinity, LENGTH - 1);
    let chosen = sidecar
        .next(&request)
        .await
        .map_err(|e| e.to_string())?
        .candidates
        .into_iter()
        .map(|candidate| candidate.id)
        .filter(|id| *id != first.id)
        .collect::<Vec<_>>();
    let mut items = vec![first];
    items.extend(client.items_by_ids(&chosen).await.map_err(describe)?);
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let hidden = store
        .call(move |w| w.store.not_interested(&ids).unwrap_or_default())
        .await
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter(|item| !hidden.contains(&item.id))
        .collect())
}

#[derive(Debug, Clone)]
pub enum Msg {
    Start(Station),
    Tuned(Station, Result<Vec<Item>, String>),
}

/// A page's Start Radio button.
pub fn start_button<'a>(station: Station) -> Element<'a, Message> {
    button(
        row![
            icon(Icon::Radio, 14.0, Tone::Accent),
            text("Start radio").wrapping(text::Wrapping::None)
        ]
        .spacing(6)
        .align_y(Center),
    )
    .style(theme::tonal)
    .padding([8, 14])
    .on_press(Message::Radio(Msg::Start(station)))
    .into()
}

impl App {
    pub(crate) fn radio(&mut self, message: Msg) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::Start(station) => {
                signed.menu = None;
                let client = signed.connection.client.clone();
                let sidecar = signed.sidecar.clone();
                let seed = station.seed.clone();
                Task::perform(tune(client, sidecar, store, seed), move |result| {
                    Message::Radio(Msg::Tuned(station.clone(), result))
                })
            }
            Msg::Tuned(station, Ok(items)) if items.is_empty() => {
                self.notice = Some(format!(
                    "There's nothing to play on {} Radio yet.",
                    station.name
                ));
                Task::none()
            }
            Msg::Tuned(station, Ok(items)) => {
                let tracks: Vec<TrackMeta> = items.iter().map(TrackMeta::from_item).collect();
                let remembered = self.remembered(Some(station.context()));
                // A station is none of the sources the history has a word for.
                Task::batch([
                    self.play(PlayRequest::from(&tracks, 0, PlaySource::Unknown)),
                    remembered,
                ])
            }
            Msg::Tuned(station, Err(error)) => {
                self.notice = Some(format!("{} Radio couldn't start: {error}", station.name));
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_json::{page, track as json_track};
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn track(id: &str) -> TrackMeta {
        TrackMeta::from_item(&Item {
            id: id.into(),
            name: "Lantern".into(),
            ..Item::default()
        })
    }

    #[test]
    fn a_menu_seeds_by_what_it_is() {
        let song = Menu::new("row".into(), vec![track("t1")]);
        assert_eq!(
            Station::of_menu(&song),
            Some(Station::new(MixSeed::Track("t1".into()), "Lantern"))
        );
        let album = Menu::for_collection(
            "a".into(),
            Collection::Album("al1".into()),
            "Low Tide".into(),
        );
        assert_eq!(
            Station::of_menu(&album).unwrap().seed,
            MixSeed::Album("al1".into())
        );
        let mut mine =
            Menu::for_collection("p".into(), Collection::Playlist("p1".into()), "Mine".into());
        assert_eq!(Station::of_menu(&mine), None, "its songs not here yet");
        mine.tracks = vec![track("t9")];
        assert_eq!(
            Station::of_menu(&mine),
            Some(Station::new(MixSeed::Track("t9".into()), "Mine"))
        );
    }

    #[test]
    fn a_station_is_remembered_so_it_can_be_seeded_again() {
        let album = Station::new(MixSeed::Album("al1".into()), "Low Tide").context();
        assert_eq!(album.kind, RecentContextKind::Station);
        assert_eq!(album.name, "Low Tide Radio");
        assert_eq!(album.seed, Some(StationSeed::Album));
        assert_eq!(album.artwork_item_id.as_deref(), Some("al1"));
        let genre = Station::new(MixSeed::Genre("Folk".into()), "Folk").context();
        assert_eq!((genre.id.as_str(), genre.seed), ("Folk", None));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_genre_station_leads_with_its_seed_and_leaves_out_not_interested() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("genre", "Folk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![json_track(1, "A")])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/aoide/next"))
            .and(body_partial_json(serde_json::json!({"seed": "1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [
                    {"id": "2", "score": 0.9, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}},
                    {"id": "3", "score": 0.8, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}}
                ],
                "profile": {"events": 0, "since": 0}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("id", "2,3"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![json_track(2, "B"), json_track(3, "C")])),
            )
            .mount(&server)
            .await;
        let client = MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let store = StoreHandle::in_memory("desk").unwrap();
        store
            .call(|w| w.store.set_not_interested(true, "2", "k", 1).unwrap())
            .await
            .unwrap();
        let songs = tune(client, Some(sidecar), store, MixSeed::Genre("Folk".into()))
            .await
            .unwrap();
        let ids: Vec<&str> = songs.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
    }
}
