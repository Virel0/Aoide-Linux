//! "A new version is out": the latest release of Aoide for Linux on GitHub,
//! compared with the running version.
//!
//! **What is sent:** one unauthenticated `GET` of
//! `https://api.github.com/repos/Virel0/Aoide-Linux/releases/latest`, with a
//! fixed `User-Agent: aoide-linux` (GitHub refuses requests without one) and
//! GitHub's API version headers. No version, no identifier, no cookie. At
//! most once a day: [`UpdateChecker::check`] reads and writes an
//! [`UpdateState`] that the app keeps in its config, and does nothing when
//! the last attempt was less than [`CHECK_INTERVAL`] ago. Whether to check
//! at all is the app's setting; nothing here runs by itself.
//!
//! pacman and Flatpak update the app themselves; this is for telling the
//! listener, with the release notes.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{StatusCode, Url};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Where the latest release is read.
pub const RELEASES_URL: &str = "https://api.github.com/repos/Virel0/Aoide-Linux/releases/latest";

/// At most one check in this long.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

const USER_AGENT: &str = "aoide-linux";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Release notes past this many characters are cut; the page has the rest.
const MAX_NOTES: usize = 20_000;

/// What the app keeps between launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateState {
    /// When the last check was attempted, in seconds since 1970, whatever
    /// came of it.
    pub last_checked: Option<u64>,
}

impl UpdateState {
    /// Whether a check may run at `now`. A last check in the future (the
    /// clock was set back) does not block one.
    pub fn is_due(&self, now: SystemTime) -> bool {
        let now = seconds(now);
        match self.last_checked {
            None => true,
            Some(last) if last > now => true,
            Some(last) => now - last >= CHECK_INTERVAL.as_secs(),
        }
    }
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release's title, if it has one.
    pub name: Option<String>,
    /// The release notes, as written (Markdown), cut at 20 000 characters.
    pub notes: String,
    /// The release page, if it is on github.com.
    pub page: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// Checked less than a day ago; nothing was sent.
    NotDue,
    /// The running version is the latest (or newer, or there is no release).
    UpToDate,
    Available(Release),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("the running version {0:?} is not a version number")]
    BadCurrentVersion(String),
    #[error("could not reach GitHub: {0}")]
    Unreachable(String),
    #[error("GitHub returned HTTP {0}")]
    Refused(u16),
    #[error("GitHub's answer was not a release with a version tag")]
    UnfamiliarReply,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: Option<String>,
    name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The version a tag names: `v1.2.3` or `1.2.3`.
pub fn tag_version(tag: &str) -> Option<Version> {
    let tag = tag.trim();
    Version::parse(tag.strip_prefix(['v', 'V']).unwrap_or(tag)).ok()
}

/// Reads GitHub's answer against the running version.
fn judge(current: &Version, body: &str) -> Result<UpdateCheck, UpdateError> {
    let raw: RawRelease = serde_json::from_str(body).map_err(|_| UpdateError::UnfamiliarReply)?;
    if raw.draft || raw.prerelease {
        return Ok(UpdateCheck::UpToDate);
    }
    let tag = raw.tag_name.ok_or(UpdateError::UnfamiliarReply)?;
    let version = tag_version(&tag).ok_or(UpdateError::UnfamiliarReply)?;
    if version <= *current {
        return Ok(UpdateCheck::UpToDate);
    }
    let page = raw.html_url.filter(|url| {
        Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"))
    });
    Ok(UpdateCheck::Available(Release {
        version,
        tag,
        name: raw
            .name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
        notes: raw
            .body
            .unwrap_or_default()
            .chars()
            .take(MAX_NOTES)
            .collect(),
        page,
    }))
}

/// Checks GitHub for a newer release. Cheap to clone.
#[derive(Debug, Clone)]
pub struct UpdateChecker {
    http: reqwest::Client,
    url: Url,
}

impl UpdateChecker {
    pub fn new() -> Result<Self, UpdateError> {
        Self::with_url(RELEASES_URL)
    }

    /// Another address answering as GitHub's API does (tests).
    pub fn with_url(url: &str) -> Result<Self, UpdateError> {
        crate::install_crypto();
        let url = Url::parse(url).map_err(|e| UpdateError::Unreachable(e.to_string()))?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| UpdateError::Unreachable(e.to_string()))?;
        Ok(Self { http, url })
    }

    /// Whether a release newer than `current` (the app's
    /// `env!("CARGO_PKG_VERSION")`) is out. Does nothing and returns
    /// [`UpdateCheck::NotDue`] if `state` says the last attempt was less than
    /// a day before `now`; otherwise records this attempt in `state` first,
    /// so a failure also waits a day.
    pub async fn check(
        &self,
        current: &str,
        state: &mut UpdateState,
        now: SystemTime,
    ) -> Result<UpdateCheck, UpdateError> {
        let current =
            Version::parse(current).map_err(|_| UpdateError::BadCurrentVersion(current.into()))?;
        if !state.is_due(now) {
            return Ok(UpdateCheck::NotDue);
        }
        state.last_checked = Some(seconds(now));
        let response = self
            .http
            .get(self.url.clone())
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| UpdateError::Unreachable(e.to_string()))?;
        match response.status() {
            // No release published yet.
            StatusCode::NOT_FOUND => return Ok(UpdateCheck::UpToDate),
            status if !status.is_success() => return Err(UpdateError::Refused(status.as_u16())),
            _ => {}
        }
        let body = response
            .text()
            .await
            .map_err(|e| UpdateError::Unreachable(e.to_string()))?;
        judge(&current, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn release(tag: &str) -> String {
        json!({
            "tag_name": tag,
            "name": " Aoide 0.2.0 ",
            "body": "## What's new\n- Tray icon",
            "html_url": "https://github.com/Virel0/Aoide-Linux/releases/tag/v0.2.0",
            "draft": false,
            "prerelease": false,
        })
        .to_string()
    }

    #[test]
    fn tags_with_or_without_a_v_are_versions() {
        assert_eq!(tag_version("v0.2.0"), Some(v("0.2.0")));
        assert_eq!(tag_version("0.2.0"), Some(v("0.2.0")));
        assert_eq!(tag_version("V1.0.0-rc.1"), Some(v("1.0.0-rc.1")));
        assert_eq!(tag_version("latest"), None);
        assert_eq!(tag_version("v1.2"), None);
    }

    #[test]
    fn a_newer_release_is_available_with_its_notes() {
        let UpdateCheck::Available(r) = judge(&v("0.1.0"), &release("v0.2.0")).unwrap() else {
            panic!("expected an update");
        };
        assert_eq!(r.version, v("0.2.0"));
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.name.as_deref(), Some("Aoide 0.2.0"));
        assert_eq!(r.notes, "## What's new\n- Tray icon");
        assert_eq!(
            r.page.as_deref(),
            Some("https://github.com/Virel0/Aoide-Linux/releases/tag/v0.2.0")
        );
    }

    #[test]
    fn the_same_or_an_older_release_is_up_to_date() {
        assert_eq!(
            judge(&v("0.2.0"), &release("v0.2.0")),
            Ok(UpdateCheck::UpToDate)
        );
        assert_eq!(
            judge(&v("0.3.0"), &release("v0.2.0")),
            Ok(UpdateCheck::UpToDate)
        );
    }

    #[test]
    fn a_release_is_newer_than_its_own_prerelease() {
        assert!(matches!(
            judge(&v("0.2.0-beta.1"), &release("v0.2.0")),
            Ok(UpdateCheck::Available(_))
        ));
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        let draft = json!({"tag_name": "v9.0.0", "draft": true}).to_string();
        let pre = json!({"tag_name": "v9.0.0", "prerelease": true}).to_string();
        assert_eq!(judge(&v("0.1.0"), &draft), Ok(UpdateCheck::UpToDate));
        assert_eq!(judge(&v("0.1.0"), &pre), Ok(UpdateCheck::UpToDate));
    }

    #[test]
    fn a_page_off_github_is_not_linked() {
        let body = json!({"tag_name": "v0.2.0", "html_url": "https://example.org/x"}).to_string();
        let Ok(UpdateCheck::Available(r)) = judge(&v("0.1.0"), &body) else {
            panic!()
        };
        assert_eq!(r.page, None);
        assert_eq!(r.name, None);
        assert_eq!(r.notes, "");
    }

    #[test]
    fn a_reply_without_a_version_tag_is_unfamiliar() {
        for body in ["not json", "{}", r#"{"tag_name":"nightly"}"#] {
            assert_eq!(
                judge(&v("0.1.0"), body),
                Err(UpdateError::UnfamiliarReply),
                "{body}"
            );
        }
    }

    #[test]
    fn long_notes_are_cut() {
        let body = json!({"tag_name": "v0.2.0", "body": "x".repeat(30_000)}).to_string();
        let Ok(UpdateCheck::Available(r)) = judge(&v("0.1.0"), &body) else {
            panic!()
        };
        assert_eq!(r.notes.chars().count(), MAX_NOTES);
    }

    #[test]
    fn a_check_is_due_once_a_day() {
        let day = CHECK_INTERVAL.as_secs();
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        assert!(UpdateState::default().is_due(at(1_000)));
        let state = UpdateState {
            last_checked: Some(10 * day),
        };
        assert!(!state.is_due(at(10 * day)));
        assert!(!state.is_due(at(11 * day - 1)));
        assert!(state.is_due(at(11 * day)));
        // The clock went back: do not wait for it to catch up.
        assert!(state.is_due(at(9 * day)));
    }
}
