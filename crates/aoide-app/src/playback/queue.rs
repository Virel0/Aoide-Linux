//! The queue as the panel shows it: Played, Now Playing, Next Up, Then.
//!
//! Two lanes after the current track, because they mean different things, as
//! on the phone: what the listener queued by hand (Play Next, Play Last) plays
//! before whatever was already running resumes. With no hand-queued tracks the
//! rest is simply Next Up.

use std::ops::Range;

use aoide_audio::{EntryId, PlayQueue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heading {
    Played,
    NowPlaying,
    NextUp,
    Then,
}

impl Heading {
    pub fn title(self) -> &'static str {
        match self {
            Heading::Played => "Played",
            Heading::NowPlaying => "Now Playing",
            Heading::NextUp => "Next Up",
            Heading::Then => "Then",
        }
    }
}

/// One heading and the entries under it, by their index in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: Heading,
    pub range: Range<usize>,
    pub entries: Vec<EntryId>,
}

/// The queue's sections in play order. Empty sections are left out.
pub fn sections<T: Clone>(queue: &PlayQueue<T>) -> Vec<Section> {
    let entries = queue.entries();
    let Some(position) = queue.position() else {
        return Vec::new();
    };
    let lane = queue.lane().len();
    let after = position + 1;
    let mut out = Vec::new();
    let mut push = |heading: Heading, range: Range<usize>| {
        if !range.is_empty() {
            out.push(Section {
                heading,
                entries: entries[range.clone()].iter().map(|e| e.id).collect(),
                range,
            });
        }
    };
    push(Heading::Played, 0..position);
    push(Heading::NowPlaying, position..after);
    push(Heading::NextUp, after..after + lane);
    let rest = if lane == 0 {
        Heading::NextUp
    } else {
        Heading::Then
    };
    push(rest, after + lane..entries.len());
    out
}

/// Where a one-step move of the entry at `index` lands, as the `destination`
/// `PlayQueue::move_entries` takes (an offset in the list before removal), or
/// `None` when it would leave its section. Moves stay inside a section so a
/// hand-queued track cannot be nudged behind the album it was queued ahead of.
pub fn step_destination(section: &Range<usize>, index: usize, up: bool) -> Option<usize> {
    if !section.contains(&index) {
        return None;
    }
    if up {
        (index > section.start).then(|| index - 1)
    } else {
        (index + 1 < section.end).then(|| index + 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(tracks: &[&'static str], index: usize) -> PlayQueue<&'static str> {
        let mut queue = PlayQueue::new();
        queue.load(tracks.to_vec(), index, false);
        queue
    }

    fn headings<T: Clone>(queue: &PlayQueue<T>) -> Vec<(Heading, usize)> {
        sections(queue)
            .into_iter()
            .map(|s| (s.heading, s.entries.len()))
            .collect()
    }

    #[test]
    fn an_empty_queue_has_no_sections() {
        assert!(sections(&PlayQueue::<&str>::new()).is_empty());
    }

    #[test]
    fn an_album_from_the_top_is_now_playing_and_next_up() {
        let queue = queue(&["a", "b", "c"], 0);
        assert_eq!(
            headings(&queue),
            [(Heading::NowPlaying, 1), (Heading::NextUp, 2)]
        );
    }

    #[test]
    fn what_has_played_sits_above() {
        let queue = queue(&["a", "b", "c", "d"], 2);
        assert_eq!(
            headings(&queue),
            [
                (Heading::Played, 2),
                (Heading::NowPlaying, 1),
                (Heading::NextUp, 1)
            ]
        );
    }

    #[test]
    fn hand_queued_tracks_are_next_up_and_the_album_is_then() {
        let mut queue = queue(&["a", "b", "c"], 0);
        queue.play_next(vec!["x"]);
        queue.append(vec!["y"]);
        let sections = sections(&queue);
        assert_eq!(
            sections
                .iter()
                .map(|s| (s.heading, s.range.clone()))
                .collect::<Vec<_>>(),
            [
                (Heading::NowPlaying, 0..1),
                (Heading::NextUp, 1..3),
                (Heading::Then, 3..5)
            ]
        );
        let lane: Vec<&str> = sections[1]
            .range
            .clone()
            .map(|i| queue.entries()[i].track)
            .collect();
        assert_eq!(lane, ["x", "y"]);
    }

    #[test]
    fn the_last_track_has_nothing_after_it() {
        let queue = queue(&["a", "b"], 1);
        assert_eq!(
            headings(&queue),
            [(Heading::Played, 1), (Heading::NowPlaying, 1)]
        );
    }

    #[test]
    fn steps_stay_inside_their_section() {
        let section = 3..6;
        assert_eq!(step_destination(&section, 3, true), None);
        assert_eq!(step_destination(&section, 4, true), Some(3));
        assert_eq!(step_destination(&section, 4, false), Some(6));
        assert_eq!(step_destination(&section, 5, false), None);
        assert_eq!(step_destination(&section, 7, false), None);
    }

    #[test]
    fn a_step_moves_one_place() {
        let mut queue = queue(&["a", "b", "c", "d"], 0);
        let next_up = sections(&queue)[1].range.clone();
        let destination = step_destination(&next_up, 1, false).unwrap();
        queue.move_entries(&[1], destination);
        let order: Vec<&str> = queue.entries().iter().map(|e| e.track).collect();
        assert_eq!(order, ["a", "c", "b", "d"]);
        let destination = step_destination(&next_up, 3, true).unwrap();
        queue.move_entries(&[3], destination);
        let order: Vec<&str> = queue.entries().iter().map(|e| e.track).collect();
        assert_eq!(order, ["a", "c", "d", "b"]);
    }
}
