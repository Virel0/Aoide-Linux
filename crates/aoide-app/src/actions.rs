//! What can be done to a song (or a set of them) from any list: the inline
//! action strip that opens under a row, and the requests it sends.

use aoide_store::PlaySource;

use crate::playback::Enqueue;
use crate::playback::tracks::TrackMeta;

/// Past this many playlists, Add to Playlist offers a search field.
pub const PLAYLIST_SEARCH_AFTER: usize = 8;

/// Play these, this way.
#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub tracks: Vec<TrackMeta>,
    pub how: Enqueue,
    pub source: PlaySource,
}

impl PlayRequest {
    pub fn from(tracks: &[TrackMeta], start: usize, source: PlaySource) -> Self {
        Self {
            tracks: tracks.to_vec(),
            how: Enqueue::Replace {
                start,
                shuffle: false,
            },
            source,
        }
    }

    pub fn shuffled(tracks: &[TrackMeta], source: PlaySource) -> Self {
        Self {
            tracks: tracks.to_vec(),
            how: Enqueue::Replace {
                start: crate::playback::random_index(tracks.len()),
                shuffle: true,
            },
            source,
        }
    }
}

/// The taste flags on one track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub not_interested: bool,
    pub dont_count: bool,
}

/// The open action strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    /// Which row it belongs to: the same song can be listed twice.
    pub anchor: String,
    pub tracks: Vec<TrackMeta>,
    /// Showing the playlists to add to.
    pub adding: bool,
    pub filter: String,
    pub new_name: String,
    /// Read from the store when the strip opens, for a single track.
    pub flags: Option<Flags>,
}

impl Menu {
    pub fn new(anchor: String, tracks: Vec<TrackMeta>) -> Self {
        Self {
            anchor,
            tracks,
            adding: false,
            filter: String::new(),
            new_name: String::new(),
            flags: None,
        }
    }

    /// The one track the flags and Go To apply to, if there is just one.
    pub fn single(&self) -> Option<&TrackMeta> {
        match self.tracks.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RowAction {
    Open {
        anchor: String,
        tracks: Vec<TrackMeta>,
    },
    Close,
    PlayNext,
    PlayLast,
    ShowPlaylists,
    Filter(String),
    AddTo(String),
    NewName(String),
    CreatePlaylist,
    NotInterested,
    DontCount,
    Favourite,
    FlagsLoaded(String, Flags),
    GoToAlbum,
    GoToArtist,
}

/// The playlists to offer, filtered by name once there are enough to need it.
pub fn matching<'a, T>(playlists: &'a [T], name: impl Fn(&T) -> &str, filter: &str) -> Vec<&'a T> {
    let filter = filter.trim().to_lowercase();
    playlists
        .iter()
        .filter(|p| filter.is_empty() || name(p).to_lowercase().contains(&filter))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_matches_names_ignoring_case() {
        let lists = ["Morning", "Late Night", "night drive"];
        let found = matching(&lists, |s| s, "NIGHT");
        assert_eq!(found, [&"Late Night", &"night drive"]);
        assert_eq!(matching(&lists, |s| s, "  ").len(), 3);
    }

    #[test]
    fn a_shuffle_starts_somewhere_inside_the_list() {
        let request = PlayRequest::shuffled(&[], PlaySource::Shuffle);
        assert!(matches!(
            request.how,
            Enqueue::Replace {
                start: 0,
                shuffle: true
            }
        ));
    }
}
