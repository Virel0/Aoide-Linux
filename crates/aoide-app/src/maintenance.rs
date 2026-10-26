//! Settings → System's housekeeping: settings exported to and imported
//! from a JSON file, the log level, and clearing caches.
//!
//! **No secrets in the file.** The export holds the settings and nothing
//! about who or where: no account, server address, device id or window
//! placement, and no token or key (those live in the system keyring and are
//! never in the config file to begin with). An import changes the same
//! settings and nothing else.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use iced::widget::{button, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task};
use serde_json::{Value, json};

use crate::app::{App, Message as AppMessage};
use crate::config::{Config, Settings};
use crate::logging::LogLevel;
use crate::system::SystemSettings;
use crate::theme;
use crate::widgets;

/// The export's format, written into the file.
pub const FORMAT: u64 = 1;
const FORMAT_KEY: &str = "aoideSettings";

/// The fields of the System settings that are about this machine's state,
/// not the listener's choices: kept out of the file, and kept on import.
const MACHINE_STATE: [&str; 3] = ["state", "available", "dismissed"];

/// The settings as a file's contents.
pub fn export(config: &Config) -> String {
    let mut system = serde_json::to_value(&config.system).unwrap_or(Value::Null);
    if let Some(updates) = system.get_mut("updates").and_then(Value::as_object_mut) {
        for key in MACHINE_STATE {
            updates.remove(key);
        }
    }
    // The output device is this machine's, not a choice that travels.
    let mut sound = serde_json::to_value(&config.sound).unwrap_or(Value::Null);
    if let Some(sound) = sound.as_object_mut() {
        sound.remove("device");
    }
    let file = json!({
        FORMAT_KEY: FORMAT,
        "settings": config.settings,
        "system": system,
        "sound": sound,
        "interface": config.interface,
        "collections": config.collections,
    });
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// `config` with the settings of `contents` in place. Fields the file does
/// not have take their defaults; the account, the device and the machine's
/// own state are kept.
pub fn import(config: &Config, contents: &str) -> Result<Config, String> {
    let file: Value =
        serde_json::from_str(contents).map_err(|_| "That file isn't settings JSON.".to_owned())?;
    match file.get(FORMAT_KEY).and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(_) => {
            return Err("Those settings come from a newer Aoide; update this one first.".into());
        }
        None => return Err("That file isn't an Aoide settings export.".into()),
    }
    let settings: Settings =
        serde_json::from_value(file.get("settings").cloned().unwrap_or(json!({})))
            .map_err(|e| format!("Its settings couldn't be read: {e}."))?;
    let mut system: SystemSettings =
        serde_json::from_value(file.get("system").cloned().unwrap_or(json!({})))
            .map_err(|e| format!("Its system settings couldn't be read: {e}."))?;
    let mut sound: crate::sound::SoundSettings =
        serde_json::from_value(file.get("sound").cloned().unwrap_or(json!({})))
            .map_err(|e| format!("Its sound settings couldn't be read: {e}."))?;
    sound.device = config.sound.device.clone();
    let interface: crate::appearance::InterfaceSettings =
        serde_json::from_value(file.get("interface").cloned().unwrap_or(json!({})))
            .map_err(|e| format!("Its appearance settings couldn't be read: {e}."))?;
    let collections: Vec<crate::screens::collections::SavedFilter> =
        serde_json::from_value(file.get("collections").cloned().unwrap_or(json!([])))
            .map_err(|e| format!("Its collections couldn't be read: {e}."))?;
    system.updates.state = config.system.updates.state;
    system.updates.available = config.system.updates.available.clone();
    system.updates.dismissed = config.system.updates.dismissed.clone();
    let mut imported = config.clone();
    imported.settings = settings;
    imported.settings.lyric_offset_ms = Settings::clamped_lyric_offset(settings.lyric_offset_ms);
    imported.system = system;
    imported.sound = sound.sanitised();
    imported.interface = interface;
    imported.collections = collections;
    Ok(imported)
}

/// Where the file goes by default: the documents folder, else home.
pub fn default_path() -> String {
    directories::UserDirs::new()
        .map(|dirs| {
            dirs.document_dir()
                .unwrap_or(dirs.home_dir())
                .join("aoide-settings.json")
        })
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Expands a leading `~/`.
fn resolve(path: &str) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(dirs) = directories::UserDirs::new()
    {
        return dirs.home_dir().join(rest);
    }
    PathBuf::from(path)
}

fn write_whole(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}

/// A cache the app keeps on disk, and what clearing it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    pub name: &'static str,
    pub dir: PathBuf,
    pub clears: &'static str,
}

/// Every cache there is to clear. The server's answers are not kept on
/// disk, so they have none.
pub fn caches(paths: &crate::config::Paths) -> Vec<Cache> {
    let mut all = vec![Cache {
        name: "Artwork",
        dir: paths.artwork_dir.clone(),
        clears: "the album covers kept on this computer; they are fetched again as they are shown",
    }];
    if let Some(dir) = crate::lyrics_online::cache_dir(paths) {
        all.push(Cache {
            name: "Internet lyrics",
            dir,
            clears: "the lyrics found on LRCLIB, and the songs it had none for; they are looked up \
                     again when played",
        });
    }
    all
}

/// Removes the files in `dir` (not the folder, nor anything under it that
/// is not a plain file). Returns how many files and bytes went.
pub fn clear_dir(dir: &Path) -> io::Result<(usize, u64)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let (mut files, mut bytes) = (0, 0);
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            fs::remove_file(entry.path())?;
            files += 1;
            bytes += meta.len();
        }
    }
    Ok((files, bytes))
}

/// Bytes in words: "12.4 MB".
pub fn size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    if bytes < 1_000_000 {
        format!("{} KB", bytes.div_ceil(1_000))
    } else {
        format!("{:.1} MB", bytes as f64 / MB)
    }
}

/// What the app holds for these sections.
#[derive(Debug, Default)]
pub struct State {
    pub path: Option<String>,
    pub file: Option<Result<String, String>>,
    pub cleared: Option<Result<String, String>>,
}

impl State {
    fn path(&self) -> String {
        self.path.clone().unwrap_or_else(default_path)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Path(String),
    Export,
    Exported(Result<String, String>),
    Import,
    Imported(Result<Box<Config>, String>),
    LogLevel(LogLevel),
    Clear(usize),
    Cleared(&'static str, Result<(usize, u64), String>),
}

impl App {
    pub(crate) fn maintenance_message(&mut self, message: Message) -> Task<AppMessage> {
        let state = &mut self.system.maintenance;
        match message {
            Message::Path(path) => {
                state.path = Some(path);
                state.file = None;
                Task::none()
            }
            Message::Export => export_task(resolve(&state.path()), export(&self.config)),
            Message::Exported(result) => {
                state.file = Some(result);
                Task::none()
            }
            Message::Import => import_task(resolve(&state.path()), self.config.clone()),
            Message::Imported(result) => {
                match result {
                    Ok(config) => {
                        self.config = *config;
                        self.system.maintenance.file = Some(Ok("Settings imported.".to_owned()));
                        self.settings_imported();
                    }
                    Err(error) => self.system.maintenance.file = Some(Err(error)),
                }
                Task::none()
            }
            Message::LogLevel(level) => {
                self.config.system.log_level = level;
                crate::logging::set_level(level);
                self.save_config();
                Task::none()
            }
            Message::Clear(index) => self.clear_cache(index),
            Message::Cleared(name, result) => {
                state.cleared = Some(match result {
                    Ok((0, _)) => Ok(format!("{name}: nothing to clear.")),
                    Ok((files, bytes)) => Ok(format!(
                        "{name}: cleared {files} file{} ({}).",
                        if files == 1 { "" } else { "s" },
                        size(bytes)
                    )),
                    Err(error) => Err(format!("{name} couldn't be cleared: {error}.")),
                });
                Task::none()
            }
        }
    }

    /// Clears the cache at `index` of [`caches`].
    fn clear_cache(&mut self, index: usize) -> Task<AppMessage> {
        let Some(cache) = self
            .paths
            .as_ref()
            .and_then(|p| caches(p).into_iter().nth(index))
        else {
            return Task::none();
        };
        if cache.name == "Artwork" {
            self.artwork.clear();
        }
        let name = cache.name;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    clear_dir(&cache.dir).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
            },
            move |r| AppMessage::System(Message::Cleared(name, r)),
        )
    }

    /// An import replaced the settings: save them and hand each to what it
    /// governs.
    fn settings_imported(&mut self) {
        self.save_config();
        let _ = self.playback.set_settings(self.config.settings.player());
        crate::logging::set_level(self.config.system.log_level);
        self.system.tray.set_enabled(self.config.system.tray.show);
        if let Some(presence) = &self.system.discord {
            presence.set_options(self.config.system.discord.clone());
        }
    }
}

/// Writes the exported settings to `path`, off the UI thread.
fn export_task(path: PathBuf, contents: String) -> Task<AppMessage> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                write_whole(&path, &contents)
                    .map(|()| format!("Saved to {}.", path.display()))
                    .map_err(|e| format!("The file couldn't be written: {e}."))
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        },
        |r| AppMessage::System(Message::Exported(r)),
    )
}

/// Reads settings from `path` over `config`, off the UI thread.
fn import_task(path: PathBuf, config: Config) -> Task<AppMessage> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let contents = fs::read_to_string(&path)
                    .map_err(|e| format!("The file couldn't be read: {e}."))?;
                import(&config, &contents).map(Box::new)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        },
        |r| AppMessage::System(Message::Imported(r)),
    )
}

/// Settings → System → Settings File, Logging and Caches.
pub fn settings_sections(app: &App) -> Element<'_, AppMessage> {
    column![file_section(app), logging_section(app), cache_section(app)]
        .spacing(24)
        .into()
}

/// The settings file: where it goes, Export and Import.
fn file_section(app: &App) -> Element<'_, AppMessage> {
    let state = &app.system.maintenance;
    let send = |m: Message| AppMessage::System(m);
    let path = state.path.clone().unwrap_or_else(default_path);
    let has_path = !path.trim().is_empty();
    let mut file_rows = column![
        text_input("Where the file goes", &path)
            .on_input(|p| AppMessage::System(Message::Path(p)))
            .padding(8)
            .style(theme::field),
        row![
            button(text("Export").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(has_path.then(|| send(Message::Export))),
            button(text("Import").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(has_path.then(|| send(Message::Import))),
        ]
        .spacing(8),
    ];
    if let Some(Ok(words) | Err(words)) = &state.file {
        file_rows = file_rows.push(widgets::secondary(words.as_str()).size(13));
    }
    crate::screens::settings::section(
        "Settings file",
        file_rows,
        "Export writes these settings to a JSON file, to keep or to bring to another computer; \
         Import reads one back. The file never holds your server, account, password, token or \
         OpenRouter key.",
    )
}

/// How much the log says.
fn logging_section(app: &App) -> Element<'_, AppMessage> {
    crate::screens::settings::section(
        "Logging",
        column![
            row![
                text("Log").width(Fill),
                pick_list(LogLevel::ALL, Some(app.config.system.log_level), |l| {
                    AppMessage::System(Message::LogLevel(l))
                })
                .text_size(13),
            ]
            .align_y(Center),
        ],
        "Aoide writes what it is doing to standard error, which your desktop session usually \
         keeps in its journal. Warnings is the default; Debugging says the most. Other parts of \
         the app never log more than warnings, and nothing logged names your server, token or \
         key.",
    )
}

/// Each cache with its Clear, and what the last one cleared.
fn cache_section(app: &App) -> Element<'_, AppMessage> {
    let state = &app.system.maintenance;
    let send = |m: Message| AppMessage::System(m);
    let mut cache_rows = column![];
    if let Some(paths) = &app.paths {
        for (index, cache) in caches(paths).into_iter().enumerate() {
            cache_rows = cache_rows.push(
                row![
                    column![
                        text(cache.name),
                        widgets::secondary(format!("Clears {}.", cache.clears)).size(12),
                    ]
                    .spacing(2)
                    .width(Fill),
                    button(text("Clear").size(13))
                        .style(theme::tonal)
                        .padding([4, 10])
                        .on_press(send(Message::Clear(index))),
                ]
                .spacing(10)
                .align_y(Center),
            );
        }
    } else {
        cache_rows = cache_rows.push(widgets::secondary(
            "This computer gives Aoide no cache folder.",
        ));
    }
    if let Some(Ok(words) | Err(words)) = &state.cleared {
        cache_rows = cache_rows.push(widgets::secondary(words.as_str()).size(13));
    }
    crate::screens::settings::section(
        "Caches",
        cache_rows,
        "The server's answers aren't kept on disk, so there is nothing of theirs to clear. Your \
         listening history and playlists are not a cache and stay.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Account;

    fn configured() -> Config {
        let mut config = Config {
            device_id: "device-1".into(),
            last_server: Some("https://music.example.net".into()),
            account: Some(Account {
                server: "https://music.example.net".into(),
                user_id: "u1".into(),
                username: "tester".into(),
            }),
            volume: 0.4,
            ..Config::default()
        };
        config.settings.crossfade = true;
        config.settings.lyric_offset_ms = 250;
        config.system.tray.show = true;
        config.system.discord.enabled = true;
        config.system.smart_search.model = "test/model".into();
        config.system.log_level = LogLevel::Info;
        config.system.updates.state.last_checked = Some(1_700_000_000);
        config.system.updates.dismissed = Some("0.2.0".into());
        config.sound.equalizer.enabled = true;
        config.sound.equalizer.choose("Loudness");
        config.sound.device = aoide_audio::DeviceChoice::Device {
            id: "hw:7".into(),
            name: "Desk Speakers".into(),
        };
        config.interface.appearance = crate::appearance::Appearance::Dark;
        config.activity = Some("gaming".into());
        config
    }

    #[test]
    fn the_export_holds_settings_and_nothing_about_who_or_where() {
        let exported = export(&configured());
        for private in [
            "music.example.net",
            "device-1",
            "tester",
            "u1",
            "last_checked",
            "dismissed",
            "Desk Speakers",
            "gaming",
        ] {
            assert!(!exported.contains(private), "{private} in {exported}");
        }
        assert!(!exported.to_lowercase().contains("token"));
        let value: Value = serde_json::from_str(&exported).unwrap();
        assert_eq!(value[FORMAT_KEY], 1);
        assert_eq!(value["settings"]["crossfade"], true);
        assert_eq!(value["system"]["tray"]["show"], true);
        assert_eq!(value["system"]["smartSearch"]["model"], "test/model");
        assert_eq!(value["system"]["logLevel"], "info");
        assert_eq!(value["system"]["updates"]["check"], true);
    }

    #[test]
    fn an_import_round_trips_and_keeps_the_machine_s_own() {
        let source = configured();
        let mut here = Config {
            device_id: "device-2".into(),
            ..Config::default()
        };
        here.sound.device = aoide_audio::DeviceChoice::Device {
            id: "hw:1".into(),
            name: "Laptop".into(),
        };
        let imported = import(&here, &export(&source)).unwrap();
        assert_eq!(imported.settings, source.settings);
        assert_eq!(imported.system.tray, source.system.tray);
        assert_eq!(imported.system.discord, source.system.discord);
        assert_eq!(imported.system.smart_search, source.system.smart_search);
        assert_eq!(imported.system.log_level, LogLevel::Info);
        assert_eq!(imported.sound.equalizer, source.sound.equalizer);
        assert_eq!(imported.interface, source.interface);
        assert_eq!(imported.collections, source.collections);
        // This machine's identity and state stay its own.
        assert_eq!(imported.sound.device, here.sound.device);
        assert_eq!(imported.activity, None);
        assert_eq!(imported.device_id, "device-2");
        assert!(imported.account.is_none());
        assert_eq!(imported.system.updates.state, here.system.updates.state);
        assert_eq!(imported.system.updates.dismissed, None);
    }

    #[test]
    fn a_wrong_file_is_refused_in_words() {
        let config = Config::default();
        assert!(
            import(&config, "{ not json")
                .unwrap_err()
                .contains("isn't settings JSON")
        );
        assert!(
            import(&config, "{}")
                .unwrap_err()
                .contains("isn't an Aoide settings export")
        );
        assert!(
            import(&config, r#"{"aoideSettings":2}"#)
                .unwrap_err()
                .contains("newer Aoide")
        );
        // Missing parts take their defaults; a wild offset is pulled back.
        let partial = import(
            &config,
            r#"{"aoideSettings":1,"settings":{"lyricOffsetMs":99999}}"#,
        )
        .unwrap();
        assert_eq!(
            partial.settings.lyric_offset_ms,
            Settings::LYRIC_OFFSET_RANGE_MS
        );
        assert_eq!(partial.system, SystemSettings::default());
    }

    #[test]
    fn exporting_writes_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("aoide-settings.json");
        write_whole(&path, &export(&configured())).unwrap();
        let back = import(&Config::default(), &fs::read_to_string(&path).unwrap()).unwrap();
        assert!(back.settings.crossfade);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn clearing_a_cache_removes_its_files_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.img"), vec![0u8; 1_500]).unwrap();
        fs::write(dir.path().join("b.img"), vec![0u8; 500]).unwrap();
        fs::create_dir(dir.path().join("keep")).unwrap();
        fs::write(dir.path().join("keep").join("c"), b"x").unwrap();
        assert_eq!(clear_dir(dir.path()).unwrap(), (2, 2_000));
        assert!(dir.path().join("keep").join("c").exists());
        assert_eq!(clear_dir(dir.path()).unwrap(), (0, 0));
        assert_eq!(clear_dir(&dir.path().join("missing")).unwrap(), (0, 0));
    }

    #[test]
    fn sizes_read_in_words() {
        assert_eq!(size(2_000), "2 KB");
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(12_400_000), "12.4 MB");
    }

    #[test]
    fn the_caches_are_artwork_and_internet_lyrics() {
        let paths = crate::config::Paths::under(Path::new("/c"), Path::new("/d"), Path::new("/k"));
        let all = caches(&paths);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].dir, Path::new("/k/artwork"));
        assert_eq!(all[1].dir, Path::new("/k/lyrics"));
    }
}
