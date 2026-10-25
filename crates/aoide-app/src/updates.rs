//! The update notice: aoide-integrations' [`UpdateChecker`] at most once a
//! day while checking is on (Settings → System → Updates), a quiet banner
//! when a newer release is out, its notes, and its page opened in the
//! browser.
//!
//! What is sent, and only while checking is on: one unauthenticated `GET`
//! of the latest release from GitHub's API with a fixed user agent, at most
//! once a day (the last attempt is kept in the config file). pacman and
//! Flatpak install updates themselves; this only tells.

use std::time::{Duration, SystemTime};

use aoide_integrations::update::{Release, UpdateCheck, UpdateChecker, UpdateState, tag_version};
use iced::widget::{Column, button, column, container, row, scrollable, text, toggler};
use iced::{Center, Element, Fill, Subscription, Task};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message as AppMessage};
use crate::theme;
use crate::widgets;

/// The running version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How often a running app asks itself whether a check is due (the checker
/// itself holds it to once a day).
const TICK: Duration = Duration::from_secs(60 * 60);

/// A development build can ask a stand-in instead of GitHub
/// (`AOIDE_DEV_UPDATE_URL=http://127.0.0.1:…/repos/…/releases/latest`).
const DEV_URL: &str = "AOIDE_DEV_UPDATE_URL";

/// A release found by a check, kept so the notice outlives the launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedRelease {
    pub version: String,
    pub name: Option<String>,
    pub notes: String,
    /// The release page on github.com.
    pub page: Option<String>,
}

impl From<&Release> for SavedRelease {
    fn from(release: &Release) -> Self {
        Self {
            version: release.version.to_string(),
            name: release.name.clone(),
            notes: release.notes.clone(),
            page: release.page.clone(),
        }
    }
}

/// The update settings and state, kept in the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    /// Ask GitHub, at most daily, whether a newer release is out.
    pub check: bool,
    pub state: UpdateState,
    /// The newer release last found.
    pub available: Option<SavedRelease>,
    /// A version the listener said "Not now" to.
    pub dismissed: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check: true,
            state: UpdateState::default(),
            available: None,
            dismissed: None,
        }
    }
}

impl UpdateSettings {
    /// The release last found, if it is newer than this build.
    pub fn newer(&self, current: &str) -> Option<&SavedRelease> {
        let current = tag_version(current)?;
        self.available
            .as_ref()
            .filter(|r| tag_version(&r.version).is_some_and(|v| v > current))
    }

    /// The release to tell the listener about: newer than this build, not
    /// dismissed, and only while checking is on.
    pub fn notice(&self, current: &str) -> Option<&SavedRelease> {
        self.newer(current)
            .filter(|r| self.check && self.dismissed.as_deref() != Some(r.version.as_str()))
    }

    /// Takes in what a check found.
    pub fn absorb(&mut self, found: &UpdateCheck) {
        match found {
            UpdateCheck::NotDue => {}
            UpdateCheck::UpToDate => self.available = None,
            UpdateCheck::Available(release) => self.available = Some(release.into()),
        }
    }
}

/// What the app holds while it runs.
#[derive(Debug, Default)]
pub struct State {
    pub checking: bool,
    /// The last check's outcome this session, in words.
    pub last: Option<Result<String, String>>,
    /// The release notes are open.
    pub notes_open: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// See whether a check is due.
    Tick,
    /// Check now, whatever the day's check said.
    CheckNow,
    Checked(Result<UpdateCheck, String>, UpdateState),
    Check(bool),
    ShowNotes(bool),
    OpenPage,
    NotNow,
}

/// The release notes as lines: headings, bullets, text. Markdown read
/// loosely: the notes are short and written for people.
pub fn note_lines(notes: &str) -> Vec<(bool, String)> {
    notes
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let trimmed = line.trim_start();
            if let Some(heading) = trimmed.strip_prefix('#') {
                (true, heading.trim_start_matches('#').trim().to_owned())
            } else if let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                (false, format!("• {item}"))
            } else {
                (false, trimmed.to_owned())
            }
        })
        .collect()
}

/// The browser, for a `https://` link: `xdg-open` on Linux (the portal in a
/// Flatpak), `open` on macOS.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only https links are opened.".to_owned());
    }
    // A development build can be pointed at a stand-in for the browser.
    let opener = match std::env::var("AOIDE_DEV_BROWSER") {
        Ok(stand_in) if cfg!(debug_assertions) => stand_in,
        _ if cfg!(target_os = "macos") => "open".to_owned(),
        _ => "xdg-open".to_owned(),
    };
    let mut child = std::process::Command::new(&opener)
        .arg(url)
        .spawn()
        .map_err(|e| format!("The browser couldn't be opened ({opener}: {e})."))?;
    // Reaped off the UI thread.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn checker() -> Result<UpdateChecker, String> {
    match std::env::var(DEV_URL) {
        Ok(url) if cfg!(debug_assertions) => UpdateChecker::with_url(&url),
        _ => UpdateChecker::new(),
    }
    .map_err(|e| e.to_string())
}

/// Asks, every hour, whether a check is due; the first tick is at launch
/// (the boot task).
pub fn ticks(settings: &UpdateSettings) -> Subscription<Message> {
    if settings.check {
        iced::time::every(TICK).map(|_| Message::Tick)
    } else {
        Subscription::none()
    }
}

impl App {
    pub(crate) fn update_message(&mut self, message: Message) -> Task<AppMessage> {
        let settings = &mut self.config.system.updates;
        let state = &mut self.system.updates;
        match message {
            Message::Tick | Message::CheckNow => {
                if !settings.check || state.checking {
                    return Task::none();
                }
                let mut asked = settings.state;
                if matches!(message, Message::CheckNow) {
                    asked = UpdateState::default();
                } else if !asked.is_due(SystemTime::now()) {
                    return Task::none();
                }
                let checker = match checker() {
                    Ok(checker) => checker,
                    Err(error) => {
                        state.last = Some(Err(error));
                        return Task::none();
                    }
                };
                state.checking = true;
                Task::perform(
                    async move {
                        let mut kept = asked;
                        let found = checker
                            .check(CURRENT, &mut kept, SystemTime::now())
                            .await
                            .map_err(|e| e.to_string());
                        (found, kept)
                    },
                    |(found, kept)| AppMessage::Update(Message::Checked(found, kept)),
                )
            }
            Message::Checked(found, kept) => {
                state.checking = false;
                settings.state = kept;
                state.last = Some(match &found {
                    Ok(found) => {
                        settings.absorb(found);
                        Ok(match found {
                            UpdateCheck::Available(r) => format!("Aoide {} is out.", r.version),
                            _ => "You have the latest version.".to_owned(),
                        })
                    }
                    Err(error) => {
                        log::warn!("update check failed: {error}");
                        Err(format!("The last check failed: {error}."))
                    }
                });
                self.save_config();
                Task::none()
            }
            Message::Check(on) => {
                settings.check = on;
                self.save_config();
                if on {
                    Task::done(AppMessage::Update(Message::Tick))
                } else {
                    Task::none()
                }
            }
            Message::ShowNotes(open) => {
                state.notes_open = open;
                Task::none()
            }
            Message::OpenPage => {
                let page = settings.notice(CURRENT).and_then(|r| r.page.clone());
                if let Some(page) = page
                    && let Err(error) = open_in_browser(&page)
                {
                    self.notice = Some(error);
                }
                Task::none()
            }
            Message::NotNow => {
                settings.dismissed = settings.available.as_ref().map(|r| r.version.clone());
                state.notes_open = false;
                self.save_config();
                Task::none()
            }
        }
    }
}

/// The quiet line across the top of the content, while a newer release is
/// out.
pub fn banner(app: &App) -> Option<Element<'_, AppMessage>> {
    let release = app.config.system.updates.notice(CURRENT)?;
    let send = |m: Message| AppMessage::Update(m);
    let line = row![
        container(text("●").size(10).style(|t| text::Style {
            color: Some(theme::accent(t)),
        }))
        .padding([0, 2]),
        text(format!("Aoide {} is out.", release.version))
            .size(13)
            .width(Fill),
        button(text("What's new").size(13))
            .style(theme::row)
            .padding([2, 8])
            .on_press(send(Message::ShowNotes(true))),
        button(text("Not now").size(13))
            .style(theme::row)
            .padding([2, 8])
            .on_press(send(Message::NotNow)),
    ]
    .spacing(8)
    .align_y(Center);
    Some(container(line).padding([6, 28]).into())
}

/// The release notes, over the window.
pub fn notes(app: &App) -> Option<Element<'_, AppMessage>> {
    if !app.system.updates.notes_open {
        return None;
    }
    let release = app.config.system.updates.notice(CURRENT)?;
    let send = |m: Message| AppMessage::Update(m);
    let title = release
        .name
        .clone()
        .unwrap_or_else(|| format!("Aoide {}", release.version));
    let mut lines: Column<'_, AppMessage> = column![].spacing(6);
    for (heading, line) in note_lines(&release.notes) {
        lines = lines.push(if heading {
            text(line).size(15).font(widgets::SEMIBOLD)
        } else {
            text(line).size(14)
        });
    }
    let mut actions = row![].spacing(8);
    if release.page.is_some() {
        actions = actions.push(
            button(text("Open release page").size(14))
                .style(theme::filled)
                .padding([6, 14])
                .on_press(send(Message::OpenPage)),
        );
    }
    actions = actions
        .push(
            button(text("Not now").size(14))
                .style(theme::tonal)
                .padding([6, 14])
                .on_press(send(Message::NotNow)),
        )
        .push(
            button(text("Close").size(14))
                .style(theme::tonal)
                .padding([6, 14])
                .on_press(send(Message::ShowNotes(false))),
        );
    let card = column![
        text(title).size(22).font(widgets::BOLD),
        widgets::secondary(format!("You have {CURRENT}.")).size(13),
        scrollable(lines).height(iced::Length::Shrink),
        widgets::secondary(
            "pacman and Flatpak install updates themselves; the release page has the rest."
        )
        .size(12),
        actions,
    ]
    .spacing(14);
    Some(crate::system::sheet(
        card.into(),
        AppMessage::Update(Message::ShowNotes(false)),
    ))
}

/// Settings → System → Updates.
pub fn settings_section<'a>(
    settings: &'a UpdateSettings,
    state: &'a State,
) -> Element<'a, AppMessage> {
    let send = |m: Message| AppMessage::Update(m);
    let mut rows = column![
        toggler(settings.check)
            .style(theme::switch)
            .label("Check for updates")
            .width(Fill)
            .on_toggle(|on| AppMessage::Update(Message::Check(on))),
        widgets::secondary(format!("This is Aoide for Linux {CURRENT}.")).size(13),
    ];
    if settings.check {
        let words = match (&state.last, settings.newer(CURRENT)) {
            _ if state.checking => "Checking…".to_owned(),
            (Some(Ok(words) | Err(words)), _) => words.clone(),
            (None, Some(release)) => format!("Aoide {} is out.", release.version),
            (None, None) if settings.state.last_checked.is_some() => {
                "No newer version at the last check.".to_owned()
            }
            (None, None) => "Not checked yet.".to_owned(),
        };
        rows = rows.push(
            row![
                widgets::secondary(words).size(13).width(Fill),
                button(text("Check now").size(13))
                    .style(theme::tonal)
                    .padding([4, 10])
                    .on_press_maybe((!state.checking).then(|| send(Message::CheckNow))),
            ]
            .align_y(Center),
        );
    }
    crate::screens::settings::section(
        "Updates",
        rows,
        "Once a day at most, Aoide asks GitHub whether a newer release is out: one request \
         for the latest release, with nothing about you or this computer in it. When there is \
         one, a line at the top of the window says so.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(version: &str) -> SavedRelease {
        SavedRelease {
            version: version.into(),
            name: None,
            notes: "## New\n- A tray icon".into(),
            page: Some("https://github.com/Virel0/aoide-linux/releases/tag/v0.2.0".into()),
        }
    }

    #[test]
    fn checking_is_on_by_default_and_has_a_switch() {
        let settings: UpdateSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.check);
        assert_eq!(settings, UpdateSettings::default());
    }

    #[test]
    fn the_notice_is_for_a_newer_release_not_dismissed_while_checking() {
        let mut settings = UpdateSettings {
            available: Some(saved("0.2.0")),
            ..UpdateSettings::default()
        };
        assert_eq!(settings.notice("0.1.0").unwrap().version, "0.2.0");
        // Updated since: no notice.
        assert!(settings.notice("0.2.0").is_none());
        assert!(settings.notice("0.3.0").is_none());
        settings.dismissed = Some("0.2.0".into());
        assert!(settings.notice("0.1.0").is_none());
        // A newer one than the dismissed one is told again.
        settings.available = Some(saved("0.3.0"));
        assert!(settings.notice("0.1.0").is_some());
        settings.check = false;
        assert!(settings.notice("0.1.0").is_none());
    }

    #[test]
    fn a_check_replaces_what_was_known() {
        let mut settings = UpdateSettings {
            available: Some(saved("0.2.0")),
            ..UpdateSettings::default()
        };
        settings.absorb(&UpdateCheck::NotDue);
        assert!(settings.available.is_some());
        settings.absorb(&UpdateCheck::UpToDate);
        assert!(settings.available.is_none());
        settings.absorb(&UpdateCheck::Available(Release {
            version: "0.4.0".parse().unwrap(),
            tag: "v0.4.0".into(),
            name: Some("Aoide 0.4".into()),
            notes: "notes".into(),
            page: None,
        }));
        let kept = settings.available.unwrap();
        assert_eq!(kept.version, "0.4.0");
        assert_eq!(kept.name.as_deref(), Some("Aoide 0.4"));
    }

    #[test]
    fn notes_read_as_headings_bullets_and_text() {
        assert_eq!(
            note_lines("## What's new\n\n- Mixes\n* Search\nThanks to everyone.\n"),
            [
                (true, "What's new".to_owned()),
                (false, "• Mixes".to_owned()),
                (false, "• Search".to_owned()),
                (false, "Thanks to everyone.".to_owned()),
            ]
        );
    }

    #[test]
    fn only_https_links_go_to_the_browser() {
        assert!(open_in_browser("file:///etc/passwd").is_err());
        assert!(open_in_browser("http://example.com").is_err());
    }

    #[test]
    fn the_running_version_is_a_version() {
        assert!(tag_version(CURRENT).is_some());
    }
}
