//! Merging one playlist into another: the source's songs appended to the
//! target in the source's order, those the target has already left out.

use std::collections::HashSet;

use super::{PlaylistError, TrackLink, UNRESOLVED_PREFIX};
use crate::Millis;
use crate::error::Result;
use crate::models::{PendingSong, Playlist, PlaylistItem, normalise};
use crate::smart::Calendar;
use crate::store::Store;

/// What a merge added to its target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Songs in the library, added as tracks.
    pub tracks: usize,
    /// Songs still waiting for the library, added as waiting entries.
    pub waiting: usize,
    /// Songs left out: the target had them, or the source named them twice.
    pub skipped: usize,
}

impl MergeReport {
    /// Every song added, waiting ones included.
    pub fn added(&self) -> usize {
        self.tracks + self.waiting
    }
}

/// One song a merge carries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Song {
    Track(TrackLink),
    Waiting(PendingSong),
}

/// When two waiting entries wait for the same song: the same request, or,
/// with no request to go by, the same title and artist.
#[derive(Debug, PartialEq, Eq, Hash)]
enum WaitKey {
    Request(i64),
    Named(String, String),
}

fn wait_key(song: &PendingSong) -> WaitKey {
    match song.request_id {
        Some(id) => WaitKey::Request(id),
        None => WaitKey::Named(
            normalise(&song.title),
            normalise(song.artist.as_deref().unwrap_or_default()),
        ),
    }
}

/// The songs of `source` the target's `existing` entries don't have, in
/// `source`'s order, each once: a track by its id or its content key (so a
/// copy whose id changed in a rescan still matches), a waiting song by
/// [`WaitKey`].
fn fresh(existing: &[PlaylistItem], source: Vec<Song>) -> Vec<Song> {
    let mut seen = Seen::default();
    for item in existing {
        match &item.pending {
            Some(song) if item.is_waiting() => seen.waiting(song),
            _ => seen.track(item.linked_jellyfin_id(), &item.content_key),
        };
    }
    source
        .into_iter()
        .filter(|song| match song {
            Song::Track(link) => seen.track(linked(&link.jellyfin_id), &link.content_key),
            Song::Waiting(pending) => seen.waiting(pending),
        })
        .collect()
}

/// The songs met so far, by every way two copies can be the same.
#[derive(Default)]
struct Seen {
    ids: HashSet<String>,
    keys: HashSet<String>,
    waits: HashSet<WaitKey>,
}

impl Seen {
    /// Notes a track, and says whether it was new.
    fn track(&mut self, id: &str, key: &str) -> bool {
        let known = self.ids.contains(id) || (!key.is_empty() && self.keys.contains(key));
        self.ids.insert(id.to_owned());
        if !key.is_empty() {
            self.keys.insert(key.to_owned());
        }
        !known
    }

    /// Notes a waiting song, and says whether it was new.
    fn waiting(&mut self, song: &PendingSong) -> bool {
        self.waits.insert(wait_key(song))
    }
}

/// A track id with the missing-track marker taken off.
fn linked(id: &str) -> &str {
    id.strip_prefix(UNRESOLVED_PREFIX).unwrap_or(id)
}

impl Store {
    /// Appends the songs of playlist `source_id` to the hand-made playlist
    /// `target_id`, in the source's order, leaving out those the target has
    /// already and the source's own repeats. Songs still waiting for the
    /// library come along as waiting entries. A smart source gives the songs
    /// its rules match at `now`. The source is left as it was. Each entry is
    /// written as [`Self::add_tracks`] and [`Self::add_waiting`] write them, so
    /// the merge syncs as any other edit.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchPlaylist`] when either playlist is not live,
    /// [`PlaylistError::MergeIntoItself`] when they are the same,
    /// [`PlaylistError::SmartTarget`] when the target is smart, an error
    /// reading a smart source's rules, or a database error.
    pub fn merge_playlist(
        &self,
        source_id: &str,
        target_id: &str,
        now: Millis,
        calendar: Calendar,
    ) -> Result<MergeReport> {
        if source_id == target_id {
            return Err(PlaylistError::MergeIntoItself.into());
        }
        let source = self.require_playlist(source_id)?;
        if self.require_playlist(target_id)?.is_smart {
            return Err(PlaylistError::SmartTarget(target_id.to_owned()).into());
        }
        let songs = self.songs_to_merge(&source, now, calendar)?;
        let offered = songs.len();
        let songs = fresh(&self.items(target_id)?, songs);
        let mut report = MergeReport {
            skipped: offered - songs.len(),
            ..MergeReport::default()
        };
        // Runs of one kind at a time, so the source's order holds across them.
        for run in songs.chunk_by(same_kind) {
            match run.first() {
                Some(Song::Track(_)) => {
                    let links: Vec<TrackLink> = run.iter().filter_map(as_track).collect();
                    report.tracks += self.add_tracks(&links, target_id, now)?.len();
                }
                Some(Song::Waiting(_)) => {
                    let pending: Vec<PendingSong> = run.iter().filter_map(as_waiting).collect();
                    report.waiting += self.add_waiting(&pending, target_id, now)?.len();
                }
                None => {}
            }
        }
        Ok(report)
    }

    /// A hand-made playlist's tracks and waiting songs in its order, or what a
    /// smart one's rules match. An entry whose track went missing is copied
    /// with its marker, so a later relink can bring it back in both playlists.
    fn songs_to_merge(
        &self,
        source: &Playlist,
        now: Millis,
        calendar: Calendar,
    ) -> Result<Vec<Song>> {
        if source.is_smart {
            let ids = self.resolve_smart(source, now, calendar)?;
            let cached = self.cached_tracks(&ids)?;
            return Ok(ids
                .iter()
                .filter_map(|id| cached.get(id))
                .map(|row| Song::Track(TrackLink::new(&row.jellyfin_id, &row.content_key)))
                .collect());
        }
        Ok(self
            .items(&source.id)?
            .into_iter()
            .map(|item| match item.pending {
                Some(song) if item.jellyfin_id.is_empty() => Song::Waiting(song),
                _ => Song::Track(TrackLink::new(item.jellyfin_id, item.content_key)),
            })
            .collect())
    }
}

fn same_kind(a: &Song, b: &Song) -> bool {
    matches!(
        (a, b),
        (Song::Track(_), Song::Track(_)) | (Song::Waiting(_), Song::Waiting(_))
    )
}

fn as_track(song: &Song) -> Option<TrackLink> {
    match song {
        Song::Track(link) => Some(link.clone()),
        Song::Waiting(_) => None,
    }
}

fn as_waiting(song: &Song) -> Option<PendingSong> {
    match song {
        Song::Waiting(pending) => Some(pending.clone()),
        Song::Track(_) => None,
    }
}
