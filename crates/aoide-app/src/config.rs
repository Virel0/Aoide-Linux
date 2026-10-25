//! What the app keeps between launches that is not a secret and not the store:
//! the device id, who is signed in to which server (never the token), the
//! window's size and position, and the Round 1 settings.
//!
//! One JSON file in the config directory, written whole through a temporary
//! file so a crash mid-write leaves the previous version, not half of one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the app's files go: XDG directories on Linux (`~/.config/aoide`,
/// `~/.local/share/aoide`, `~/.cache/aoide`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub store_file: PathBuf,
    pub artwork_dir: PathBuf,
}

impl Paths {
    /// The per-user directories, or `None` on a system with no home directory.
    pub fn system() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("io.github", "Virel0", "Aoide")?;
        Some(Self::under(
            dirs.config_dir(),
            dirs.data_dir(),
            dirs.cache_dir(),
        ))
    }

    pub fn under(config: &Path, data: &Path, cache: &Path) -> Self {
        Self {
            config_file: config.join("config.json"),
            store_file: data.join("aoide.sqlite"),
            artwork_dir: cache.join("artwork"),
        }
    }
}

/// The settings TRACKER.md lists for Round 1, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Fade one song into the next. Off by default, as on the phone.
    pub crossfade: bool,
    /// Consecutive tracks of one album run straight into each other whatever
    /// Crossfade says.
    pub keep_albums_whole: bool,
    /// Keep the queue going when it runs out.
    pub infinity: bool,
    /// Play only the measured span of each track.
    pub skip_silence: bool,
    /// Turn loud records down to −18 LUFS.
    pub level_volume: bool,
    /// Lyrics shown this much later (positive) or earlier (negative).
    pub lyric_offset_ms: i32,
}

impl Settings {
    /// The slider's range on the phone: a second and a half either way.
    pub const LYRIC_OFFSET_RANGE_MS: i32 = 1_500;

    pub fn player(&self) -> aoide_audio::PlayerSettings {
        aoide_audio::PlayerSettings {
            crossfade: self.crossfade,
            album_lock: self.keep_albums_whole,
        }
    }

    /// The offset held to the slider's range.
    pub fn clamped_lyric_offset(offset_ms: i32) -> i32 {
        offset_ms.clamp(-Self::LYRIC_OFFSET_RANGE_MS, Self::LYRIC_OFFSET_RANGE_MS)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            crossfade: false,
            keep_albums_whole: true,
            infinity: false,
            skip_silence: true,
            level_volume: true,
            lyric_offset_ms: 0,
        }
    }
}

/// Who is signed in, to which server. The token lives in the system keyring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The normalised server address.
    pub server: String,
    pub user_id: String,
    pub username: String,
}

impl Account {
    /// The keyring entry's name: one per user per server.
    pub fn keyring_key(&self) -> String {
        format!("{}@{}", self.user_id, self.server)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    /// `None` until the window has been moved once; the window manager places it.
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl WindowGeometry {
    pub const MIN_WIDTH: f32 = 900.0;
    pub const MIN_HEIGHT: f32 = 600.0;

    /// A size the window can open at: never smaller than the minimum, never a
    /// value a corrupt file could make absurd.
    pub fn sanitised(self) -> Self {
        let default = Self::default();
        let size = |value: f32, min: f32, fallback: f32| {
            if value.is_finite() && value >= min && value <= 16_384.0 {
                value
            } else {
                fallback
            }
        };
        let place = |value: Option<f32>| value.filter(|v| v.is_finite() && v.abs() <= 32_768.0);
        Self {
            width: size(self.width, Self::MIN_WIDTH, default.width),
            height: size(self.height, Self::MIN_HEIGHT, default.height),
            x: place(self.x),
            y: place(self.y),
        }
    }
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            width: 1_280.0,
            height: 820.0,
            x: None,
            y: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Generated once; Jellyfin keys sessions and playback reports off it.
    pub device_id: String,
    /// What the sign-in screen offers, kept after signing out.
    pub last_server: Option<String>,
    pub account: Option<Account>,
    pub window: WindowGeometry,
    pub settings: Settings,
    /// Master volume, 0 to 1.
    pub volume: f32,
    /// The equaliser and the compressor.
    pub sound: crate::sound::SoundSettings,
    /// Settings → General: the window title, the look.
    pub interface: crate::appearance::InterfaceSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            last_server: None,
            account: None,
            window: WindowGeometry::default(),
            settings: Settings::default(),
            volume: 1.0,
            sound: crate::sound::SoundSettings::default(),
            interface: crate::appearance::InterfaceSettings::default(),
        }
    }
}

impl Config {
    /// Reads the config at `path`. A missing file is a first launch; an
    /// unreadable one is treated the same way rather than stopping the app,
    /// since everything in it can be set again. The device id is minted here
    /// the first time, so it exists before anything uses it.
    pub fn load(path: &Path) -> Self {
        let mut config: Config = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if config.device_id.trim().is_empty() {
            config.device_id = uuid::Uuid::new_v4().simple().to_string();
        }
        config.window = config.window.sanitised();
        config.settings.lyric_offset_ms =
            Settings::clamped_lyric_offset(config.settings.lyric_offset_ms);
        if !config.volume.is_finite() {
            config.volume = 1.0;
        }
        config.volume = config.volume.clamp(0.0, 1.0);
        config.sound = config.sound.sanitised();
        config
    }

    /// Writes the whole file through a sibling temporary file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_the_phones_defaults() {
        let settings = Settings::default();
        assert!(!settings.crossfade);
        assert!(settings.keep_albums_whole);
        assert!(!settings.infinity);
        assert!(settings.skip_silence);
        assert!(settings.level_volume);
        assert_eq!(settings.lyric_offset_ms, 0);
    }

    #[test]
    fn a_config_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let config = Config {
            device_id: "device-1".into(),
            last_server: Some("https://music.example.net".into()),
            account: Some(Account {
                server: "https://music.example.net".into(),
                user_id: "u1".into(),
                username: "alice".into(),
            }),
            window: WindowGeometry {
                width: 1_400.0,
                height: 900.0,
                x: Some(40.0),
                y: Some(60.0),
            },
            settings: Settings {
                crossfade: true,
                keep_albums_whole: false,
                infinity: true,
                skip_silence: false,
                level_volume: false,
                lyric_offset_ms: -250,
            },
            volume: 0.5,
            sound: {
                let mut sound = crate::sound::SoundSettings::default();
                sound.equalizer.enabled = true;
                sound.equalizer.choose("Loudness");
                sound.compressor.enabled = true;
                sound
            },
            interface: crate::appearance::InterfaceSettings {
                track_in_title: false,
                appearance: crate::appearance::Appearance::Dark,
                accent: crate::appearance::Accent::Teal,
                internet_lyrics: false,
            },
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path), config);
    }

    #[test]
    fn a_first_launch_mints_a_device_id_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let first = Config::load(&path);
        assert!(!first.device_id.is_empty());
        first.save(&path).unwrap();
        assert_eq!(Config::load(&path).device_id, first.device_id);
    }

    #[test]
    fn a_corrupt_file_is_a_first_launch_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, b"{ not json").unwrap();
        let config = Config::load(&path);
        assert_eq!(config.settings, Settings::default());
        assert!(config.account.is_none());
    }

    #[test]
    fn fields_missing_from_an_older_file_take_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, br#"{"deviceId":"d","settings":{"crossfade":true}}"#).unwrap();
        let config = Config::load(&path);
        assert_eq!(config.device_id, "d");
        assert!(config.settings.crossfade);
        assert!(config.settings.keep_albums_whole);
        assert_eq!(config.volume, 1.0);
    }

    #[test]
    fn out_of_range_values_are_pulled_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            br#"{"deviceId":"d","volume":7,"window":{"width":10,"height":1e9},"settings":{"lyricOffsetMs":99999}}"#,
        )
        .unwrap();
        let config = Config::load(&path);
        assert_eq!(config.volume, 1.0);
        assert_eq!(config.window.width, WindowGeometry::default().width);
        assert_eq!(config.window.height, WindowGeometry::default().height);
        assert_eq!(
            config.settings.lyric_offset_ms,
            Settings::LYRIC_OFFSET_RANGE_MS
        );
    }

    #[test]
    fn the_config_file_never_holds_a_token_field() {
        let json = serde_json::to_string(&Config::default()).unwrap();
        assert!(!json.to_lowercase().contains("token"), "{json}");
    }

    #[test]
    fn paths_follow_the_given_directories() {
        let paths = Paths::under(Path::new("/c"), Path::new("/d"), Path::new("/k"));
        assert_eq!(paths.config_file, Path::new("/c/config.json"));
        assert_eq!(paths.store_file, Path::new("/d/aoide.sqlite"));
        assert_eq!(paths.artwork_dir, Path::new("/k/artwork"));
    }
}
