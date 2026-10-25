//! The app's log: lines on standard error (which a desktop session usually
//! keeps in its journal), at the level Settings → System → Logging names.
//!
//! The level governs Aoide's own messages. Other crates' messages are held
//! to warnings and errors whatever it says: their debug output (an HTTP
//! client's, say) can carry request URLs, and a stream URL carries the
//! access token. Aoide's own messages never name a server address, a token
//! or a key.

use std::io::Write;

use log::{Level, LevelFilter, Log, Metadata, Record};
use serde::{Deserialize, Serialize};

/// The levels Settings offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    #[default]
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    pub const ALL: [LogLevel; 5] = [
        LogLevel::Off,
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Debug,
    ];

    pub fn filter(self) -> LevelFilter {
        match self {
            LogLevel::Off => LevelFilter::Off,
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Debug => LevelFilter::Debug,
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LogLevel::Off => "Off",
            LogLevel::Error => "Errors",
            LogLevel::Warn => "Warnings",
            LogLevel::Info => "Information",
            LogLevel::Debug => "Debugging",
        })
    }
}

/// Whether a line from `target` at `level` is written under `chosen`.
pub fn allowed(target: &str, level: Level, chosen: LevelFilter) -> bool {
    let ours = target == "aoide" || target.starts_with("aoide::") || target.starts_with("aoide_");
    let ceiling = if ours {
        chosen
    } else {
        chosen.min(LevelFilter::Warn)
    };
    level <= ceiling
}

struct Stderr;

impl Log for Stderr {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        allowed(metadata.target(), metadata.level(), log::max_level())
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

static LOGGER: Stderr = Stderr;

/// Installs the logger at `level`. A second call only changes the level.
pub fn init(level: LogLevel) {
    let _ = log::set_logger(&LOGGER);
    set_level(level);
}

pub fn set_level(level: LogLevel) {
    log::set_max_level(level.filter());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_governs_aoides_lines() {
        assert!(allowed("aoide::tray", Level::Info, LevelFilter::Info));
        assert!(!allowed("aoide::tray", Level::Debug, LevelFilter::Info));
        assert!(allowed(
            "aoide_jellyfin::client",
            Level::Debug,
            LevelFilter::Debug
        ));
        assert!(!allowed("aoide", Level::Error, LevelFilter::Off));
    }

    #[test]
    fn other_crates_never_go_past_warnings() {
        assert!(!allowed(
            "reqwest::connect",
            Level::Debug,
            LevelFilter::Debug
        ));
        assert!(!allowed("hyper_util", Level::Info, LevelFilter::Debug));
        assert!(allowed("wgpu_core", Level::Warn, LevelFilter::Debug));
        assert!(!allowed("wgpu_core", Level::Warn, LevelFilter::Error));
        // A crate whose name only starts like ours is not ours.
        assert!(!allowed("aoidex", Level::Info, LevelFilter::Info));
    }

    #[test]
    fn warnings_by_default() {
        assert_eq!(LogLevel::default(), LogLevel::Warn);
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"debug\"").unwrap(),
            LogLevel::Debug
        );
    }
}
