//! A song whose artist is only a guess, found in the library by its title alone.
//!
//! A YouTube video whose title doesn't name its artist is credited to its channel,
//! which may be a label's ("Ubisoft Music") rather than the artist's. The library's
//! copy, credited to the real artist, then fails the matcher's artist test, and the
//! song would be reported missing and asked for again. So a track marked
//! [`ImportedTrack::artist_from_channel`] that nothing matched is looked up once
//! more by title: when every library song with exactly that title (as
//! [`TrackMatcher::normalize`] spells it) is by the same artist, that song is it,
//! and the track takes the library's artists, so the server's own match, when the
//! playlist is saved, finds the same song. Songs of that title by different artists
//! are no answer: which one the video is can't be told. The phone's
//! `TrackMatcher+TitleAlone.swift` follows the same rule.

use super::{
    ImportMatch, ImportedTrack, LibrarySearch, LibraryTrack, MatchedTrack, SEARCH_LIMIT,
    SEARCH_TERM_CHARS, TrackMatcher,
};

/// The candidate `track` is by its title alone, when its artist is only a guess
/// and every candidate with exactly its title is by the same (first) artist.
pub(crate) fn by_title_alone(track: &ImportedTrack, candidates: &[LibraryTrack]) -> Option<usize> {
    if !track.artist_from_channel {
        return None;
    }
    let title = TrackMatcher::normalize(&track.title);
    let mut same = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| TrackMatcher::normalize(&candidate.title) == title);
    let (first, chosen) = same.next()?;
    let artist = |candidate: &LibraryTrack| {
        candidate
            .artists
            .first()
            .map(|name| TrackMatcher::normalize(name))
    };
    let by = artist(chosen);
    same.all(|(_, other)| artist(other) == by).then_some(first)
}

/// Looks each unmatched track whose artist is only a guess up by its title alone,
/// and takes the library's song, and its artists, for it when there's one.
pub(super) async fn rescue<S: LibrarySearch>(library: &S, matches: &mut [ImportMatch]) {
    for each in matches
        .iter_mut()
        .filter(|each| each.found.is_none() && each.track.artist_from_channel)
    {
        for term in TrackMatcher::search_terms(&each.track.title) {
            let term: String = term.chars().take(SEARCH_TERM_CHARS).collect();
            let Ok(candidates) = library.search_songs(&term, SEARCH_LIMIT).await else {
                continue;
            };
            let Some(index) = by_title_alone(&each.track, &candidates) else {
                continue;
            };
            let chosen = &candidates[index];
            each.track.artists.clone_from(&chosen.artists);
            each.track.artist_from_channel = false;
            each.found = Some(MatchedTrack {
                jellyfin_id: chosen.jellyfin_id.clone(),
                title: Some(chosen.title.clone()),
                artists: chosen.artists.clone(),
                album: chosen.album.clone(),
                duration_ms: chosen.duration_ms,
                score: TrackMatcher::verdict(&each.track, chosen).overall,
            });
            break;
        }
    }
}
