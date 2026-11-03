//! A request and a playlist import in words: where each stands, and the finer
//! points under that.

use aoide_melete::{
    Copying, MusicRequest, PlaylistImportSummary, PlaylistImportTrack, Progress, RequestKind,
    RequestState,
};

use crate::screens::sync_status::ago;

pub fn kind_word(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Artist => "Artist",
        RequestKind::Album => "Album",
        RequestKind::Song => "Song",
        RequestKind::Other => "Request",
    }
}

/// Where a request stands, in words.
pub fn standing(request: &MusicRequest) -> String {
    match request.state {
        RequestState::Requested => "Waiting for approval".to_owned(),
        RequestState::Declined => match &request.reason {
            Some(reason) => format!("Declined: {reason}"),
            None => "Declined".to_owned(),
        },
        RequestState::Approved => request
            .progress
            .as_ref()
            .map_or_else(|| "Approved".to_owned(), progress),
    }
}

fn progress(progress: &Progress) -> String {
    let from = match progress.source.as_deref() {
        Some("soulseek") => " from Soulseek",
        Some("torrent") => " by torrent",
        _ => "",
    };
    match (progress.step.as_str(), progress.percent) {
        ("downloading", Some(percent)) => format!("Downloading{from}, {percent:.0}%"),
        ("downloading", None) => format!("Downloading{from}"),
        ("wanted", _) => "Approved, waiting to be looked for".to_owned(),
        ("searching", _) => "Looking for a download".to_owned(),
        ("transferring", _) => match &progress.copied {
            Some(copied) => format!("Copying to your server: {}", copying(copied)),
            None => "Copying to your server".to_owned(),
        },
        ("importing", _) => "Adding to your library".to_owned(),
        ("in_library", _) => "In your library".to_owned(),
        ("failed", _) => progress.message.clone().map_or_else(
            || "Couldn't be fetched".to_owned(),
            |m| format!("Couldn't be fetched: {m}"),
        ),
        (other, _) => progress.message.clone().unwrap_or_else(|| other.to_owned()),
    }
}

/// "120 of 400 MB, 12 MB/s, about 24 seconds left".
fn copying(copied: &Copying) -> String {
    let mut words = format!("{} of {}", size(copied.bytes), size(copied.total));
    if let Some(speed) = copied.bytes_per_second {
        words.push_str(&format!(", {}/s", size(speed)));
    }
    match copied.seconds_left {
        None => {}
        Some(0..60) => words.push_str(", under a minute left"),
        Some(seconds) => {
            let minutes = seconds.div_ceil(60);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            words.push_str(&format!(", about {minutes} {unit} left"));
        }
    }
    words
}

/// Bytes in the largest unit that keeps a figure: "3.4 GB", "120 MB".
#[allow(clippy::cast_precision_loss, reason = "a figure shown to one decimal")]
fn size(bytes: u64) -> String {
    const UNITS: [(&str, f64); 3] = [("GB", 1e9), ("MB", 1e6), ("KB", 1e3)];
    for (unit, scale) in UNITS {
        let value = bytes as f64 / scale;
        if value >= 10.0 {
            return format!("{value:.0} {unit}");
        }
        if value >= 1.0 {
            return format!("{value:.1} {unit}");
        }
    }
    format!("{bytes} bytes")
}

/// The finer points under where a request stands: Melete's latest word when it
/// says more, who decided and when, and when it last changed.
pub fn detail(request: &MusicRequest, now_secs: i64) -> Option<String> {
    let when = |secs: i64| ago(secs * 1000, now_secs * 1000);
    let mut parts = Vec::new();
    let step = request.progress.as_ref().map(|p| p.step.as_str());
    if let Some(progress) = &request.progress
        && let Some(message) = &progress.message
        && !matches!(step, Some("failed" | "in_library"))
        && !standing(request)
            .to_lowercase()
            .contains(&message.to_lowercase())
    {
        parts.push(message.clone());
    }
    match (request.state, &request.decided_by, request.decided_at) {
        (RequestState::Requested, _, _) => {
            parts.push(format!("Asked {}", when(request.created_at)))
        }
        (RequestState::Approved, Some(by), Some(at)) => {
            parts.push(format!("Approved by {by} {}", when(at)));
        }
        (RequestState::Approved, None, Some(at)) => {
            parts.push(format!("Approved {}", when(at)));
        }
        (RequestState::Declined, Some(by), Some(at)) => {
            parts.push(format!("Declined by {by} {}", when(at)));
        }
        _ => {}
    }
    if let Some(at) = request.progress.as_ref().and_then(|p| p.updated_at)
        && step != Some("in_library")
    {
        parts.push(format!("Updated {}", when(at)));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// An import's tally: "Done: 12 asked for, 3 asked already, 2 not found. 5 wait for approval."
pub fn import_tally(import: &PlaylistImportSummary) -> String {
    let state = match import.state.as_str() {
        "queued" => "Waiting to start",
        "running" => "Asking",
        "failed" => "Stopped",
        _ => "Done",
    };
    let mut words = format!(
        "{state}: {} of {} songs asked for, {} asked already, {} not found.",
        import.asked, import.tracks, import.already, import.unmatched
    );
    if import.waiting > 0 {
        words.push_str(&format!(" {} wait for approval.", import.waiting));
    }
    if let Some(message) = &import.message {
        words.push(' ');
        words.push_str(message);
    }
    words
}

/// What became of one track of an import.
pub fn track_standing(track: &PlaylistImportTrack) -> String {
    match (track.state.as_str(), &track.why) {
        ("asked", _) => "Asked for".to_owned(),
        ("already", _) => "Asked for already".to_owned(),
        ("unmatched", Some(why)) => format!("Not found: {why}"),
        ("unmatched", None) => "Not found".to_owned(),
        _ => "Not looked up yet".to_owned(),
    }
}

/// Whether an import is still working through its tracks.
pub fn import_running(import: &PlaylistImportSummary) -> bool {
    matches!(import.state.as_str(), "queued" | "running")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_010_000;

    fn request(state: RequestState, progress: Option<(&str, Option<f64>)>) -> MusicRequest {
        MusicRequest {
            id: 1,
            kind: RequestKind::Album,
            title: "Low Tide".into(),
            year: None,
            artist: None,
            state,
            reason: (state == RequestState::Declined).then(|| "not on any tracker".into()),
            created_at: NOW - 7_200,
            decided_at: None,
            decided_by: None,
            progress: progress.map(|(step, percent)| Progress {
                step: step.into(),
                percent,
                message: Some("No release fits the profile".into()),
                source: None,
                updated_at: None,
                copied: None,
            }),
        }
    }

    #[test]
    fn a_request_says_where_it_stands() {
        let standing_of = |state, progress| standing(&request(state, progress));
        assert_eq!(
            standing_of(RequestState::Requested, None),
            "Waiting for approval"
        );
        assert_eq!(
            standing_of(RequestState::Declined, None),
            "Declined: not on any tracker"
        );
        assert_eq!(
            standing_of(RequestState::Approved, Some(("downloading", Some(42.4)))),
            "Downloading, 42%"
        );
        assert_eq!(
            standing_of(RequestState::Approved, Some(("in_library", None))),
            "In your library"
        );
        assert_eq!(
            standing_of(RequestState::Approved, Some(("failed", None))),
            "Couldn't be fetched: No release fits the profile"
        );
    }

    #[test]
    fn a_download_names_its_source_and_a_copy_its_progress() {
        let mut downloading = request(RequestState::Approved, Some(("downloading", Some(10.0))));
        downloading.progress.as_mut().unwrap().source = Some("soulseek".into());
        assert_eq!(standing(&downloading), "Downloading from Soulseek, 10%");
        let mut copying = request(RequestState::Approved, Some(("transferring", None)));
        copying.progress.as_mut().unwrap().copied = Some(Copying {
            bytes: 120_000_000,
            total: 3_400_000_000,
            bytes_per_second: Some(12_000_000),
            seconds_left: Some(275),
        });
        assert_eq!(
            standing(&copying),
            "Copying to your server: 120 MB of 3.4 GB, 12 MB/s, about 5 minutes left"
        );
    }

    #[test]
    fn the_detail_says_who_decided_when_and_the_latest_word() {
        assert_eq!(
            detail(&request(RequestState::Requested, None), NOW).as_deref(),
            Some("Asked 2 hours ago")
        );
        let mut searching = request(RequestState::Approved, Some(("searching", None)));
        searching.decided_by = Some("admin".into());
        searching.decided_at = Some(NOW - 3_600);
        searching.progress.as_mut().unwrap().updated_at = Some(NOW - 120);
        assert_eq!(
            detail(&searching, NOW).as_deref(),
            Some(
                "No release fits the profile \u{b7} Approved by admin 1 hour ago \u{b7} Updated 2 minutes ago"
            )
        );
        let mut failed = request(RequestState::Approved, Some(("failed", None)));
        failed.decided_at = Some(NOW - 30);
        assert_eq!(
            detail(&failed, NOW).as_deref(),
            Some("Approved just now"),
            "the failure's message is in the standing already"
        );
    }

    #[test]
    fn an_import_tallies_its_tracks() {
        let import = PlaylistImportSummary {
            id: 4,
            name: "Road trip".into(),
            state: "done".into(),
            message: None,
            created_at: NOW,
            tracks: 20,
            asked: 12,
            already: 3,
            unmatched: 5,
            waiting: 4,
            in_library: 0,
            playlist_id: None,
        };
        assert_eq!(
            import_tally(&import),
            "Done: 12 of 20 songs asked for, 3 asked already, 5 not found. 4 wait for approval."
        );
        assert!(!import_running(&import));
    }
}
