//! Discord Rich Presence: aoide-integrations' [`Presence`] on the desktop
//! integration's runtime, fed from what plays while Settings → System →
//! Discord is on, and its settings.
//!
//! The presence task is started once and, while the setting is off, opens no
//! socket. Nothing is handed to it while off either: what plays reaches it
//! only when the listener has turned it on.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use aoide_integrations::discord::{
    DisplayType, Endpoint, NowPlaying as Playing, Presence, PresenceOptions, PresenceStatus, Track,
};
use aoide_melete::Url;
use iced::futures::{Stream, StreamExt};
use iced::widget::{column, pick_list, row, text};
use iced::{Center, Element, Fill, Subscription, Task};
use tokio::sync::watch;

use crate::app::{App, Message as AppMessage};
use crate::integration::NowPlaying;

/// A development build can talk to a stand-in socket instead of looking for
/// Discord (`AOIDE_DEV_DISCORD_SOCKET=/path/to/socket`).
const DEV_SOCKET: &str = "AOIDE_DEV_DISCORD_SOCKET";

#[derive(Debug, Clone)]
pub enum Message {
    Enabled(bool),
    Display(Display),
    ShowWhenPaused(bool),
    StateIcon(bool),
    Links(bool),
    Status(PresenceStatus),
}

/// The display types, as the picker names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Display(pub DisplayType);

impl Display {
    pub const ALL: [Display; 3] = [
        Display(DisplayType::AppName),
        Display(DisplayType::Artist),
        Display(DisplayType::Song),
    ];
}

impl std::fmt::Display for Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            DisplayType::AppName => "Listening to Aoide",
            DisplayType::Artist => "Listening to the artist",
            DisplayType::Song => "Listening to the song",
        })
    }
}

/// The settings screen's line for where the presence stands.
pub fn status_words(status: PresenceStatus) -> &'static str {
    match status {
        PresenceStatus::Off => "",
        PresenceStatus::Searching => "Discord isn't running. Aoide looks for it every 15 seconds.",
        PresenceStatus::Connected => "Connected.",
        PresenceStatus::Unsupported => "Rich Presence isn't available on this system.",
    }
}

/// Where to look for Discord.
fn endpoint() -> Endpoint {
    match std::env::var_os(DEV_SOCKET) {
        Some(path) if cfg!(debug_assertions) => Endpoint::Paths(vec![PathBuf::from(path)]),
        _ => Endpoint::Discover,
    }
}

/// Starts the presence task on `runtime` with `options`; it connects to
/// nothing until they are enabled.
pub fn start(
    runtime: Option<&tokio::runtime::Handle>,
    options: PresenceOptions,
) -> Option<Presence> {
    let _inside = runtime?.enter();
    Some(Presence::spawn(endpoint(), options))
}

/// A search on MusicBrainz, the open music encyclopaedia: a public page
/// with no tracking, for the song and artist lines when links are on.
fn musicbrainz_search(query: &str, kind: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    Url::parse_with_params(
        "https://musicbrainz.org/search",
        [("query", query), ("type", kind)],
    )
    .ok()
    .map(String::from)
}

/// What plays, as the presence describes it. `cover` is the server's image
/// address for the track, or `None`; the presence uses it only if it is a
/// public `https` URL without credentials, and only with Show Cover on.
pub fn playing(
    now: &NowPlaying,
    cover: Option<String>,
    links: bool,
    at: SystemTime,
) -> Option<Playing> {
    now.track_id.as_ref()?;
    let artists: Vec<String> = now
        .artist
        .split(", ")
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect();
    let first_artist = artists.first().cloned().unwrap_or_default();
    Some(Playing {
        track: Track {
            title: now.title.clone(),
            album: Some(now.album.trim().to_owned()).filter(|a| !a.is_empty()),
            length: now.length_ms.map(Duration::from_millis),
            cover_url: cover,
            song_url: links
                .then(|| musicbrainz_search(&format!("{} {first_artist}", now.title), "recording"))
                .flatten(),
            artist_url: links
                .then(|| musicbrainz_search(&first_artist, "artist"))
                .flatten(),
            artists,
        },
        playing: now.playing,
        position: Duration::from_millis(now.position_ms),
        at,
    })
}

/// The presence's status as it changes.
#[derive(Clone)]
struct StatusSource(watch::Receiver<PresenceStatus>);

impl Hash for StatusSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "aoide-discord-status".hash(state);
    }
}

pub fn status(presence: Option<&Presence>) -> Subscription<Message> {
    match presence {
        Some(presence) => {
            Subscription::run_with(StatusSource(presence.subscribe_status()), stream_status)
        }
        None => Subscription::none(),
    }
}

fn stream_status(source: &StatusSource) -> impl Stream<Item = Message> + use<> {
    iced::futures::stream::unfold(source.0.clone(), |mut status| async move {
        status.changed().await.ok()?;
        let now = *status.borrow_and_update();
        Some((Message::Status(now), status))
    })
    .boxed()
}

impl App {
    pub(crate) fn discord_message(&mut self, message: Message) -> Task<AppMessage> {
        let options = &mut self.config.system.discord;
        match message {
            Message::Enabled(on) => options.enabled = on,
            Message::Display(display) => options.display = display.0,
            Message::ShowWhenPaused(on) => options.show_when_paused = on,
            Message::StateIcon(on) => options.show_state_icon = on,
            Message::Links(on) => options.show_links = on,
            Message::Status(status) => {
                log::info!("Discord Rich Presence: {status:?}");
                self.system.discord_status = status;
                return Task::none();
            }
        }
        if let Some(presence) = &self.system.discord {
            presence.set_options(self.config.system.discord.clone());
        }
        self.save_config();
        // Turned on: hand over what is already playing.
        self.publish_discord();
        Task::none()
    }

    /// What plays, to the presence, only while it is on.
    pub(crate) fn publish_discord(&self) {
        let options = &self.config.system.discord;
        let Some(presence) = self.system.discord.as_ref().filter(|_| options.enabled) else {
            return;
        };
        let now = self.desktop.now_playing();
        // No cover: Melete's covers need a signed-in user, so Discord couldn't
        // fetch one, and its address would name the server.
        presence.set_now_playing(playing(&now, None, options.show_links, SystemTime::now()));
    }
}

/// Settings → System → Discord.
pub fn settings_section<'a>(
    options: &PresenceOptions,
    status: PresenceStatus,
) -> Element<'a, AppMessage> {
    let switch = |label: &'a str, value: bool, message: fn(bool) -> Message| {
        crate::widgets::switch(label, value, move |on| AppMessage::Discord(message(on)))
    };
    let mut rows = column![switch("Rich Presence", options.enabled, Message::Enabled)];
    if options.enabled {
        rows = rows
            .push(
                row![
                    text("Status shows").width(Fill),
                    pick_list(Display::ALL, Some(Display(options.display)), |d| {
                        AppMessage::Discord(Message::Display(d))
                    })
                    .text_size(13),
                ]
                .align_y(Center),
            )
            .push(switch(
                "Show when paused",
                options.show_when_paused,
                Message::ShowWhenPaused,
            ))
            .push(switch(
                "Playing and paused badge",
                options.show_state_icon,
                Message::StateIcon,
            ))
            .push(switch("Links", options.show_links, Message::Links));
        let words = status_words(status);
        if !words.is_empty() {
            rows = rows.push(crate::widgets::secondary(words).size(13));
        }
    }
    crate::screens::settings::section(
        "Discord",
        rows,
        "Rich Presence shows the song, artist, album and a time bar on your Discord profile, \
         through the Discord app on this computer; nothing is sent while it is off. The cover is \
         your server's own address for the image, so everyone who sees your status can see that \
         address: it is sent only when your server is on a public https address and the link \
         carries no login, and Aoide's icon stands in otherwise. Links open a MusicBrainz search \
         for the song and the artist.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_integrations::discord::activity;

    fn now() -> NowPlaying {
        NowPlaying {
            track_id: Some("t1".into()),
            title: "Harbour Lights".into(),
            artist: "The Quiet Engines, Mara Vey".into(),
            album: "Low Tide".into(),
            length_ms: Some(243_000),
            position_ms: 61_000,
            playing: true,
            ..NowPlaying::default()
        }
    }

    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    #[test]
    fn nothing_plays_nothing_is_described() {
        assert_eq!(playing(&NowPlaying::default(), None, true, at()), None);
    }

    #[test]
    fn the_track_is_described_from_what_the_desktop_sees() {
        let described = playing(&now(), None, false, at()).unwrap();
        assert_eq!(described.track.title, "Harbour Lights");
        assert_eq!(described.track.artists, ["The Quiet Engines", "Mara Vey"]);
        assert_eq!(described.track.album.as_deref(), Some("Low Tide"));
        assert_eq!(described.track.length, Some(Duration::from_millis(243_000)));
        assert_eq!(described.position, Duration::from_millis(61_000));
        assert!(described.playing);
        assert_eq!(described.at, at());
        assert_eq!(described.track.song_url, None);
        assert_eq!(described.track.artist_url, None);
    }

    #[test]
    fn links_are_musicbrainz_searches_and_survive_the_presence_rules() {
        let described = playing(&now(), None, true, at()).unwrap();
        let song = described.track.song_url.clone().unwrap();
        assert_eq!(
            song,
            "https://musicbrainz.org/search?query=Harbour+Lights+The+Quiet+Engines&type=recording"
        );
        assert_eq!(
            described.track.artist_url.as_deref(),
            Some("https://musicbrainz.org/search?query=The+Quiet+Engines&type=artist")
        );
        let options = PresenceOptions {
            enabled: true,
            show_links: true,
            ..PresenceOptions::default()
        };
        let sent = activity(Some(&described), &options).unwrap();
        assert_eq!(sent["details_url"], song);
        assert!(sent["state_url"].as_str().is_some());
    }

    #[test]
    fn a_lan_cover_never_leaves_but_a_public_one_does() {
        let options = PresenceOptions {
            enabled: true,
            show_cover: true,
            ..PresenceOptions::default()
        };
        let lan = playing(
            &now(),
            Some("http://192.0.2.10:8096/Items/a1/Images/Primary?tag=x".into()),
            false,
            at(),
        );
        let sent = activity(lan.as_ref(), &options).unwrap();
        assert_eq!(sent["assets"]["large_image"], "icon");
        let public = "https://music.example.net/Items/a1/Images/Primary?maxWidth=512&tag=x";
        let sent = activity(
            playing(&now(), Some(public.into()), false, at()).as_ref(),
            &options,
        )
        .unwrap();
        assert_eq!(sent["assets"]["large_image"], public);
    }

    #[test]
    fn every_status_has_words_but_off() {
        assert_eq!(status_words(PresenceStatus::Off), "");
        assert_eq!(status_words(PresenceStatus::Connected), "Connected.");
        assert!(status_words(PresenceStatus::Searching).starts_with("Discord isn't running"));
        assert!(!status_words(PresenceStatus::Unsupported).is_empty());
    }

    #[test]
    fn the_options_default_off_as_in_the_electron_app() {
        let options: PresenceOptions = serde_json::from_str("{}").unwrap();
        assert!(!options.enabled);
        assert_eq!(options, PresenceOptions::default());
    }
}
