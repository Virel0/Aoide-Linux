//! Start Radio: Jellyfin's Instant Mix seeded by a song, an album, an artist,
//! a genre or a playlist (`instant_mix_from`), without the songs marked Not
//! Interested, played at once and remembered as a station for the resume grid.

use aoide_jellyfin::{Item, JellyfinClient, MixSeed};
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
    /// playlist. An Aoide playlist is not a Jellyfin item, so its first song
    /// seeds it.
    pub fn of_menu(menu: &Menu) -> Option<Self> {
        if let Some(track) = menu.single() {
            return Some(Self::new(MixSeed::Track(track.id.clone()), &track.title));
        }
        let (collection, title) = menu.collection.as_ref()?;
        let seed = match collection {
            Collection::Album(id) => MixSeed::Album(id.clone()),
            Collection::Artist(id) => MixSeed::Artist(id.clone()),
            Collection::JellyfinPlaylist(id) => MixSeed::Playlist(id.clone()),
            Collection::Playlist(_) => MixSeed::Track(menu.tracks.first()?.id.clone()),
        };
        Some(Self::new(seed, title))
    }

    fn id(&self) -> &str {
        match &self.seed {
            MixSeed::Track(id)
            | MixSeed::Album(id)
            | MixSeed::Artist(id)
            | MixSeed::Genre(id)
            | MixSeed::Playlist(id) => id,
        }
    }

    /// What the resume grid keeps: an album's or artist's station says what
    /// seeded it so it is seeded the same way again; the others are resumed
    /// with the kind-blind Instant Mix of the same item.
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

/// The station's songs in the server's order, Not Interested ones left out.
pub async fn tune(
    client: JellyfinClient,
    store: StoreHandle,
    seed: MixSeed,
) -> Result<Vec<Item>, String> {
    let items = client
        .instant_mix_from(&seed, LENGTH)
        .await
        .map_err(|e| crate::failure::describe(&e).to_string())?;
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
                let seed = station.seed.clone();
                Task::perform(tune(client, store, seed), move |result| {
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
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn track(id: &str) -> TrackMeta {
        TrackMeta::from_item(
            &serde_json::from_value(
                serde_json::json!({"Id": id, "Name": "Lantern", "Type": "Audio"}),
            )
            .unwrap(),
        )
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
        let genre = Station::new(MixSeed::Genre("g1".into()), "Folk").context();
        assert_eq!((genre.id.as_str(), genre.seed), ("g1", None));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_interested_songs_stay_off_the_station() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/MusicGenres/InstantMix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [
                    {"Id": "a", "Name": "A", "Type": "Audio"},
                    {"Id": "b", "Name": "B", "Type": "Audio"},
                    {"Id": "c", "Name": "C", "Type": "Audio"}
                ],
                "TotalRecordCount": 3
            })))
            .expect(1)
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
        let store = StoreHandle::in_memory("desk").unwrap();
        store
            .call(|w| w.store.set_not_interested(true, "b", "k", 1).unwrap())
            .await
            .unwrap();
        let songs = tune(client, store, MixSeed::Genre("g1".into()))
            .await
            .unwrap();
        let ids: Vec<&str> = songs.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }
}
