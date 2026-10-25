//! What the presence says for a given track and options: the `activity`
//! object of Discord's `SET_ACTIVITY` command. Pure, so every rule is tested
//! without a socket.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::links::{clean_link, public_https};

/// Discord's activity type for "Listening to".
const LISTENING: u8 = 2;
/// The asset key of Aoide's icon in the Discord application.
pub const ICON_ASSET: &str = "icon";
/// Asset keys of the small play-state badges.
pub const PLAYING_ASSET: &str = "playing";
pub const PAUSED_ASSET: &str = "paused";
/// Discord refuses text fields outside 2..=128 characters.
const MIN_TEXT: usize = 2;
const MAX_TEXT: usize = 128;

/// What the member list shows after "Listening to".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayType {
    /// "Listening to Aoide".
    #[default]
    AppName,
    /// "Listening to <artist>".
    Artist,
    /// "Listening to <song>".
    Song,
}

impl DisplayType {
    /// Discord's `status_display_type`: 0 name, 1 state, 2 details.
    fn wire(self) -> u8 {
        match self {
            Self::AppName => 0,
            Self::Artist => 1,
            Self::Song => 2,
        }
    }
}

/// The settings behind the presence. Kept by the app (its config file) and
/// handed to [`super::Presence`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PresenceOptions {
    /// Nothing reaches Discord, not even a connection, unless this is on.
    pub enabled: bool,
    pub display: DisplayType,
    /// Keep the presence up while paused (without the time bar). Off clears
    /// it on pause.
    pub show_when_paused: bool,
    /// Use the track's cover as the large image. The URL is visible to
    /// everyone who can see the presence, so only a public `https://` URL
    /// without credentials is used; anything else falls back to Aoide's icon.
    pub show_cover: bool,
    /// The small playing / paused badge on the image.
    pub show_state_icon: bool,
    /// Make the song and artist lines links, where the app supplies clean
    /// URLs (see [`Track::song_url`]).
    pub show_links: bool,
}

impl Default for PresenceOptions {
    /// As the Electron app ships it: off, "Aoide", shown when paused, no
    /// cover, with the state badge, no links.
    fn default() -> Self {
        Self {
            enabled: false,
            display: DisplayType::AppName,
            show_when_paused: true,
            show_cover: false,
            show_state_icon: true,
            show_links: false,
        }
    }
}

/// The track as the presence describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub length: Option<Duration>,
    /// A public cover image. See [`PresenceOptions::show_cover`].
    pub cover_url: Option<String>,
    /// A public page for the song (MusicBrainz, say). Used only with
    /// [`PresenceOptions::show_links`], only if `https://`, and dropped if it
    /// carries credentials or tracking parameters (`utm_*`, `si`, `fbclid`...).
    pub song_url: Option<String>,
    /// The same for the artist line.
    pub artist_url: Option<String>,
}

/// What plays now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub track: Track,
    pub playing: bool,
    /// Position in the track at `at`.
    pub position: Duration,
    /// When `position` was read; the time bar is drawn from it.
    pub at: SystemTime,
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Discord's limits: at most 128 characters, at least 2. Short text is
/// padded with a no-break space rather than dropped.
fn fit(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut chars: Vec<char> = text.chars().collect();
    if chars.len() > MAX_TEXT {
        chars.truncate(MAX_TEXT - 1);
        chars.push('…');
    }
    while chars.len() < MIN_TEXT {
        chars.push('\u{a0}');
    }
    Some(chars.into_iter().collect())
}

/// The `activity` for `now` under `options`, or `None` to clear the presence
/// (disabled, nothing playing, or paused with `show_when_paused` off).
pub fn activity(now: Option<&NowPlaying>, options: &PresenceOptions) -> Option<Value> {
    if !options.enabled {
        return None;
    }
    let now = now?;
    if !now.playing && !options.show_when_paused {
        return None;
    }
    let track = &now.track;
    let mut activity = Map::new();
    activity.insert("type".into(), json!(LISTENING));
    activity.insert("status_display_type".into(), json!(options.display.wire()));
    let details = fit(&track.title).unwrap_or_else(|| "Unknown song".into());
    activity.insert("details".into(), json!(details));
    if let Some(state) = fit(&track.artists.join(", ")) {
        activity.insert("state".into(), json!(state));
    }
    if options.show_links {
        if let Some(url) = track.song_url.as_deref().and_then(clean_link) {
            activity.insert("details_url".into(), json!(url));
        }
        if let Some(url) = track.artist_url.as_deref().and_then(clean_link) {
            activity.insert("state_url".into(), json!(url));
        }
    }

    if now.playing {
        let start =
            millis(now.at).saturating_sub(u64::try_from(now.position.as_millis()).unwrap_or(0));
        let mut timestamps = Map::new();
        timestamps.insert("start".into(), json!(start));
        if let Some(length) = track.length.filter(|l| !l.is_zero()) {
            let end = start.saturating_add(u64::try_from(length.as_millis()).unwrap_or(0));
            timestamps.insert("end".into(), json!(end));
        }
        activity.insert("timestamps".into(), Value::Object(timestamps));
    }

    let mut assets = Map::new();
    let cover = options
        .show_cover
        .then(|| track.cover_url.as_deref().and_then(public_https))
        .flatten();
    let large = cover.map_or_else(|| ICON_ASSET.to_owned(), String::from);
    assets.insert("large_image".into(), json!(large));
    if let Some(album) = track.album.as_deref().and_then(fit) {
        assets.insert("large_text".into(), json!(album));
    }
    if options.show_state_icon {
        let (image, text) = if now.playing {
            (PLAYING_ASSET, "Playing")
        } else {
            (PAUSED_ASSET, "Paused")
        };
        assets.insert("small_image".into(), json!(image));
        assets.insert("small_text".into(), json!(text));
    }
    activity.insert("assets".into(), Value::Object(assets));
    activity.insert("instance".into(), json!(false));
    Some(Value::Object(activity))
}

/// How far the time bar may move before it is worth telling Discord. A
/// position read a little late is not a seek.
const TIMESTAMP_TOLERANCE_MS: u64 = 2_000;

/// Whether two activities would look the same in Discord: equal except for
/// timestamps within [`TIMESTAMP_TOLERANCE_MS`].
pub(crate) fn same_activity(a: Option<&Value>, b: Option<&Value>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return a.is_none() && b.is_none();
    };
    let stamps = |v: &Value, key: &str| {
        v.get("timestamps")
            .and_then(|t| t.get(key))
            .and_then(Value::as_u64)
    };
    for key in ["start", "end"] {
        match (stamps(a, key), stamps(b, key)) {
            (Some(x), Some(y)) if x.abs_diff(y) <= TIMESTAMP_TOLERANCE_MS => {}
            (None, None) => {}
            _ => return false,
        }
    }
    let without = |v: &Value| {
        let mut v = v.clone();
        if let Some(map) = v.as_object_mut() {
            map.remove("timestamps");
        }
        v
    };
    without(a) == without(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> PresenceOptions {
        PresenceOptions {
            enabled: true,
            ..PresenceOptions::default()
        }
    }

    fn now(playing: bool) -> NowPlaying {
        NowPlaying {
            track: Track {
                title: "Song".into(),
                artists: vec!["One".into(), "Two".into()],
                album: Some("Album".into()),
                length: Some(Duration::from_secs(200)),
                cover_url: Some("https://coverartarchive.org/release/x/front.jpg".into()),
                song_url: Some("https://musicbrainz.org/recording/x".into()),
                artist_url: Some("https://musicbrainz.org/artist/y?utm_source=aoide".into()),
            },
            playing,
            position: Duration::from_secs(30),
            at: UNIX_EPOCH + Duration::from_secs(1_000_000),
        }
    }

    #[test]
    fn nothing_is_said_while_disabled() {
        assert_eq!(
            activity(Some(&now(true)), &PresenceOptions::default()),
            None
        );
    }

    #[test]
    fn nothing_playing_clears() {
        assert_eq!(activity(None, &on()), None);
    }

    #[test]
    fn a_playing_track_is_listening_with_a_time_bar() {
        let a = activity(Some(&now(true)), &on()).unwrap();
        assert_eq!(a["type"], 2);
        assert_eq!(a["status_display_type"], 0);
        assert_eq!(a["details"], "Song");
        assert_eq!(a["state"], "One, Two");
        assert_eq!(a["timestamps"]["start"], 1_000_000_000 - 30_000);
        assert_eq!(a["timestamps"]["end"], 1_000_000_000 - 30_000 + 200_000);
        assert_eq!(a["assets"]["large_image"], "icon");
        assert_eq!(a["assets"]["large_text"], "Album");
        assert_eq!(a["assets"]["small_image"], "playing");
        assert_eq!(a["assets"]["small_text"], "Playing");
        assert!(a.get("details_url").is_none(), "links are off by default");
    }

    #[test]
    fn the_display_type_picks_the_member_list_line() {
        let wire = |display| {
            let options = PresenceOptions { display, ..on() };
            activity(Some(&now(true)), &options).unwrap()["status_display_type"].clone()
        };
        assert_eq!(wire(DisplayType::AppName), 0);
        assert_eq!(wire(DisplayType::Artist), 1);
        assert_eq!(wire(DisplayType::Song), 2);
    }

    #[test]
    fn paused_keeps_the_presence_without_a_time_bar_or_clears_it() {
        let a = activity(Some(&now(false)), &on()).unwrap();
        assert!(a.get("timestamps").is_none());
        assert_eq!(a["assets"]["small_image"], "paused");

        let options = PresenceOptions {
            show_when_paused: false,
            ..on()
        };
        assert_eq!(activity(Some(&now(false)), &options), None);
    }

    #[test]
    fn the_state_badge_can_be_turned_off() {
        let options = PresenceOptions {
            show_state_icon: false,
            ..on()
        };
        let a = activity(Some(&now(true)), &options).unwrap();
        assert!(a["assets"].get("small_image").is_none());
        assert!(a["assets"].get("small_text").is_none());
    }

    #[test]
    fn the_cover_is_used_only_when_asked_and_public() {
        let options = PresenceOptions {
            show_cover: true,
            ..on()
        };
        let a = activity(Some(&now(true)), &options).unwrap();
        assert_eq!(
            a["assets"]["large_image"],
            "https://coverartarchive.org/release/x/front.jpg"
        );

        for url in [
            "https://jellyfin.example.org/Items/1/Images/Primary?api_key=secret",
            "http://jellyfin.example.org/Items/1/Images/Primary",
            "https://user:pass@example.org/cover.jpg",
            "file:///home/someone/.cache/cover.jpg",
        ] {
            let mut playing = now(true);
            playing.track.cover_url = Some(url.into());
            let a = activity(Some(&playing), &options).unwrap();
            assert_eq!(a["assets"]["large_image"], "icon", "{url}");
        }
    }

    #[test]
    fn links_are_kept_only_when_clean() {
        let options = PresenceOptions {
            show_links: true,
            ..on()
        };
        let a = activity(Some(&now(true)), &options).unwrap();
        assert_eq!(a["details_url"], "https://musicbrainz.org/recording/x");
        // The artist link carries utm_source, so it is left out.
        assert!(a.get("state_url").is_none());
    }

    #[test]
    fn text_fits_discords_limits() {
        let mut playing = now(true);
        playing.track.title = "x".repeat(300);
        playing.track.artists = vec!["A".into()];
        playing.track.album = Some("  ".into());
        let a = activity(Some(&playing), &on()).unwrap();
        let details = a["details"].as_str().unwrap();
        assert_eq!(details.chars().count(), 128);
        assert!(details.ends_with('…'));
        assert_eq!(a["state"].as_str().unwrap().chars().count(), 2);
        assert!(a["assets"].get("large_text").is_none());
    }

    #[test]
    fn an_untitled_track_still_says_something() {
        let mut playing = now(true);
        playing.track.title = " ".into();
        playing.track.artists.clear();
        let a = activity(Some(&playing), &on()).unwrap();
        assert_eq!(a["details"], "Unknown song");
        assert!(a.get("state").is_none());
    }

    #[test]
    fn a_track_without_a_length_has_no_end() {
        let mut playing = now(true);
        playing.track.length = None;
        let a = activity(Some(&playing), &on()).unwrap();
        assert!(a["timestamps"].get("end").is_none());
    }

    #[test]
    fn small_drift_in_the_time_bar_is_the_same_activity() {
        let a = activity(Some(&now(true)), &on());
        let mut later = now(true);
        later.at += Duration::from_secs(10);
        later.position += Duration::from_millis(10_800);
        let b = activity(Some(&later), &on());
        assert!(same_activity(a.as_ref(), b.as_ref()));

        // A seek is not.
        later.position += Duration::from_secs(20);
        let c = activity(Some(&later), &on());
        assert!(!same_activity(a.as_ref(), c.as_ref()));

        // Nor is pausing, or clearing.
        let paused = activity(Some(&now(false)), &on());
        assert!(!same_activity(a.as_ref(), paused.as_ref()));
        assert!(!same_activity(a.as_ref(), None));
        assert!(same_activity(None, None));
    }
}
