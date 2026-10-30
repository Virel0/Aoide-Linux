//! What was actually listened to, into the local store.
//!
//! The play events sync carries to Melete, which keeps the household's
//! history from them. A play is opened when a track is heard to start and closed when the
//! next one starts or playback stops, with the furthest point reached, and
//! the store's classifier decides whether that was a play or a skip.
//!
//! Runs on the store thread (see [`crate::store_worker`]), so every call is
//! plain blocking code.

use aoide_audio::EntryId;
use aoide_store::{Activity, Millis, PlayEvent, PlaySource, Store, TrackRef};

#[derive(Debug, Clone, PartialEq)]
struct OpenPlay {
    event: PlayEvent,
    entry: EntryId,
    duration_ms: Option<i64>,
    /// A high-water mark: dragging back to the second verse does not undo the
    /// three minutes already heard.
    ms_played: i64,
    /// The engine said the track was heard to its end (a natural end or a
    /// crossfade out), which the classifier takes as the player's word.
    ended: bool,
}

/// At most one play is open, because at most one track is playing.
#[derive(Debug, Default)]
pub struct PlayRecorder {
    open: Option<OpenPlay>,
    /// The entry last started, counted or not, so a seek (a restart of the same
    /// entry with no end between) is not a new play.
    current: Option<EntryId>,
    current_ended: bool,
}

impl PlayRecorder {
    /// A track was heard to start. Closes what was open, caches the track's row,
    /// and opens a play unless the track is flagged Don't Count Plays.
    ///
    /// The same entry starting again without having ended is a seek or a
    /// restart, which continues the play rather than beginning another; after
    /// a natural end (repeat one) it is a new play.
    pub fn started(
        &mut self,
        store: &Store,
        entry: EntryId,
        track: &TrackRef,
        source: PlaySource,
        activity: Option<Activity>,
        now: Millis,
    ) -> aoide_store::Result<()> {
        if self.current == Some(entry) && !self.current_ended {
            return Ok(());
        }
        self.close(store, now)?;
        self.current = Some(entry);
        self.current_ended = false;

        // Not fatal to the play: a history row without a cached track still counts.
        let _ = store.cache_tracks(std::slice::from_ref(track));

        if store
            .flags(&track.jellyfin_id)?
            .is_some_and(|flags| flags.dont_count)
        {
            return Ok(());
        }
        let event = store.begin_play(
            &track.jellyfin_id,
            &track.content_key,
            source,
            activity,
            now,
        )?;
        self.open = Some(OpenPlay {
            event,
            entry,
            duration_ms: track.duration_ms,
            ms_played: 0,
            ended: false,
        });
        Ok(())
    }

    /// The heard position in `entry`.
    pub fn progress(&mut self, entry: EntryId, ms: u64) {
        if let Some(play) = self.open.as_mut().filter(|play| play.entry == entry) {
            play.ms_played = play.ms_played.max(i64::try_from(ms).unwrap_or(i64::MAX));
        }
    }

    /// `entry` was heard to its end.
    pub fn ended(&mut self, entry: EntryId) {
        if self.current == Some(entry) {
            self.current_ended = true;
        }
        if let Some(play) = self.open.as_mut().filter(|play| play.entry == entry) {
            play.ended = true;
        }
    }

    /// Closes the open play, if any: playback stopped, the queue was replaced,
    /// or the next track started. Returns the event as amended.
    pub fn close(&mut self, store: &Store, now: Millis) -> aoide_store::Result<Option<PlayEvent>> {
        let Some(play) = self.open.take() else {
            return Ok(None);
        };
        let completed = play.ended.then_some(true);
        store
            .finish_play(
                &play.event,
                play.ms_played,
                play.duration_ms,
                now,
                completed,
            )
            .map(Some)
    }

    /// Playback stopped: the next start of any entry is a new play.
    pub fn stopped(
        &mut self,
        store: &Store,
        now: Millis,
    ) -> aoide_store::Result<Option<PlayEvent>> {
        self.current = None;
        self.current_ended = false;
        self.close(store, now)
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::in_memory("device-1").unwrap()
    }

    fn track(id: &str, duration_ms: i64) -> TrackRef {
        let mut row = TrackRef::new(id, format!("key-{id}"), id, "Artist", "Album", 0);
        row.duration_ms = Some(duration_ms);
        row
    }

    fn events(store: &Store, id: &str) -> Vec<(i64, bool, bool)> {
        store
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT msPlayed, completed, skipped FROM playEvent WHERE jellyfinId = ? ORDER BY startedAt",
                )?;
                let rows = statement.query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect()
            })
            .unwrap()
    }

    #[test]
    fn a_skip_and_a_listen_are_told_apart() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        recorder
            .started(
                &store,
                EntryId(1),
                &track("a", 200_000),
                PlaySource::Album,
                None,
                1_000,
            )
            .unwrap();
        recorder.progress(EntryId(1), 4_000);
        recorder
            .started(
                &store,
                EntryId(2),
                &track("b", 200_000),
                PlaySource::Album,
                None,
                5_000,
            )
            .unwrap();
        recorder.progress(EntryId(2), 150_000);
        recorder.stopped(&store, 160_000).unwrap();

        assert_eq!(events(&store, "a"), [(4_000, false, true)]);
        assert_eq!(events(&store, "b"), [(150_000, false, false)]);
        assert_eq!(store.play_count("b").unwrap(), 1);
        assert_eq!(store.play_count("a").unwrap(), 0);
    }

    #[test]
    fn the_furthest_point_counts_not_the_last() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        recorder
            .started(
                &store,
                EntryId(1),
                &track("a", 300_000),
                PlaySource::Album,
                None,
                0,
            )
            .unwrap();
        recorder.progress(EntryId(1), 180_000);
        recorder.progress(EntryId(1), 20_000);
        recorder.stopped(&store, 1).unwrap();
        assert_eq!(events(&store, "a")[0].0, 180_000);
    }

    #[test]
    fn the_activity_set_when_a_play_begins_is_written_on_it() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        let a = track("a", 300_000);
        recorder
            .started(
                &store,
                EntryId(1),
                &a,
                PlaySource::Album,
                Some(Activity::Focus),
                0,
            )
            .unwrap();
        recorder.stopped(&store, 1).unwrap();
        let tag: Option<String> = store
            .read(|conn| {
                conn.query_row(
                    "SELECT activity FROM playEvent WHERE jellyfinId = 'a'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(tag.as_deref(), Some("focus"));
    }

    #[test]
    fn a_seek_continues_the_play() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        let a = track("a", 300_000);
        recorder
            .started(&store, EntryId(1), &a, PlaySource::Album, None, 0)
            .unwrap();
        recorder.progress(EntryId(1), 30_000);
        // A seek is a load of the same entry: the engine reports a start again.
        recorder
            .started(&store, EntryId(1), &a, PlaySource::Album, None, 1)
            .unwrap();
        recorder.progress(EntryId(1), 200_000);
        recorder.stopped(&store, 2).unwrap();
        assert_eq!(events(&store, "a"), [(200_000, false, false)]);
    }

    #[test]
    fn repeat_one_is_a_new_play_each_time_round() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        let a = track("a", 100_000);
        recorder
            .started(&store, EntryId(1), &a, PlaySource::Album, None, 0)
            .unwrap();
        recorder.progress(EntryId(1), 99_500);
        recorder.ended(EntryId(1));
        recorder
            .started(&store, EntryId(1), &a, PlaySource::Album, None, 100_000)
            .unwrap();
        recorder.progress(EntryId(1), 10_000);
        recorder.stopped(&store, 110_000).unwrap();
        assert_eq!(
            events(&store, "a"),
            [(99_500, true, false), (10_000, false, true)]
        );
    }

    #[test]
    fn a_crossfaded_end_is_complete_on_the_players_word() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        recorder
            .started(
                &store,
                EntryId(1),
                &track("a", 200_000),
                PlaySource::Album,
                None,
                0,
            )
            .unwrap();
        // Faded out eight seconds early.
        recorder.progress(EntryId(1), 192_000);
        recorder.ended(EntryId(1));
        recorder
            .started(
                &store,
                EntryId(2),
                &track("b", 200_000),
                PlaySource::Album,
                None,
                1,
            )
            .unwrap();
        assert_eq!(events(&store, "a"), [(192_000, true, false)]);
    }

    #[test]
    fn dont_count_opens_no_play() {
        let store = store();
        store.set_dont_count(true, "a", "key-a", 0).unwrap();
        let mut recorder = PlayRecorder::default();
        recorder
            .started(
                &store,
                EntryId(1),
                &track("a", 200_000),
                PlaySource::Album,
                None,
                0,
            )
            .unwrap();
        assert!(!recorder.is_open());
        recorder.progress(EntryId(1), 150_000);
        recorder.stopped(&store, 1).unwrap();
        assert!(events(&store, "a").is_empty());
        // The track is still cached, so the Hidden from mixes list can name it.
        assert_eq!(store.cached_tracks(&["a".into()]).unwrap().len(), 1);
    }

    #[test]
    fn progress_for_another_entry_is_ignored() {
        let store = store();
        let mut recorder = PlayRecorder::default();
        recorder
            .started(
                &store,
                EntryId(1),
                &track("a", 200_000),
                PlaySource::Album,
                None,
                0,
            )
            .unwrap();
        recorder.progress(EntryId(9), 150_000);
        recorder.stopped(&store, 1).unwrap();
        assert_eq!(events(&store, "a")[0].0, 0);
    }
}
