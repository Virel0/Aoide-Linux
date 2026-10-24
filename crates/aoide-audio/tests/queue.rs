//! `PlayQueueTests.swift`, ported case for case.

use aoide_audio::queue::{AdvanceOutcome, EntryId, PlayQueue, RepeatMode};

fn tracks(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn titles(queue: &PlayQueue<String>) -> Vec<String> {
    queue.entries().iter().map(|e| e.track.clone()).collect()
}

fn current(queue: &PlayQueue<String>) -> Option<&str> {
    queue.current_track().map(String::as_str)
}

fn ids(entries: &[aoide_audio::queue::QueueEntry<String>]) -> Vec<&str> {
    entries.iter().map(|e| e.track.as_str()).collect()
}

fn loaded(names: &[&str], index: usize) -> PlayQueue<String> {
    let mut queue = PlayQueue::new();
    queue.load(tracks(names), index, false);
    queue
}

// Loading

#[test]
fn an_empty_queue_has_no_current_track() {
    let queue: PlayQueue<String> = PlayQueue::new();
    assert!(queue.is_empty());
    assert!(queue.current().is_none());
    assert_eq!(queue.position(), None);
}

#[test]
fn loading_starts_at_the_requested_index() {
    let queue = loaded(&["a", "b", "c"], 1);
    assert_eq!(current(&queue), Some("b"));
    assert_eq!(ids(queue.up_next()), ["c"]);
}

#[test]
fn an_out_of_range_start_index_is_clamped() {
    assert_eq!(current(&loaded(&["a", "b"], 99)), Some("b"));
}

#[test]
fn the_same_song_twice_stays_two_slots() {
    let queue = loaded(&["a", "a"], 0);
    assert_ne!(queue.entries()[0].id, queue.entries()[1].id);
}

// Advancing

#[test]
fn auto_advance_walks_forward() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    assert_eq!(queue.advance_automatically(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("b"));
}

#[test]
fn auto_advance_off_the_end_stops_when_repeat_is_off() {
    assert_eq!(
        loaded(&["a", "b"], 1).advance_automatically(),
        AdvanceOutcome::Ended
    );
}

#[test]
fn repeat_all_wraps_to_the_start() {
    let mut queue = loaded(&["a", "b"], 1);
    queue.repeat_mode = RepeatMode::All;
    assert_eq!(queue.advance_automatically(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn repeat_one_replays_on_a_natural_end() {
    let mut queue = loaded(&["a", "b"], 0);
    queue.repeat_mode = RepeatMode::One;
    assert_eq!(queue.advance_automatically(), AdvanceOutcome::RepeatCurrent);
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn next_escapes_repeat_one() {
    let mut queue = loaded(&["a", "b"], 0);
    queue.repeat_mode = RepeatMode::One;
    assert_eq!(queue.skip_to_next(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("b"));
}

#[test]
fn next_on_the_last_track_with_repeat_one_wraps() {
    let mut queue = loaded(&["a", "b"], 1);
    queue.repeat_mode = RepeatMode::One;
    assert_eq!(queue.skip_to_next(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn previous_steps_back() {
    let mut queue = loaded(&["a", "b"], 1);
    assert_eq!(queue.skip_to_previous(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn previous_from_the_first_track_reports_ended() {
    let mut queue = loaded(&["a", "b"], 0);
    assert_eq!(queue.skip_to_previous(), AdvanceOutcome::Ended);
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn previous_from_the_first_track_wraps_when_repeating() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    queue.repeat_mode = RepeatMode::All;
    assert_eq!(queue.skip_to_previous(), AdvanceOutcome::Moved);
    assert_eq!(current(&queue), Some("c"));
}

// Shuffle

#[test]
fn shuffling_keeps_the_current_song_and_puts_it_first() {
    let mut queue = loaded(&["a", "b", "c", "d"], 2);
    queue.set_shuffled(true);
    assert_eq!(current(&queue), Some("c"));
    assert_eq!(queue.position(), Some(0));
    assert_eq!(queue.entries().len(), 4);
}

#[test]
fn unshuffling_restores_the_order_and_finds_the_current_song() {
    let mut queue = loaded(&["a", "b", "c", "d"], 2);
    queue.set_shuffled(true);
    queue.set_shuffled(false);
    assert_eq!(titles(&queue), tracks(&["a", "b", "c", "d"]));
    assert_eq!(current(&queue), Some("c"));
    assert_eq!(queue.position(), Some(2));
}

#[test]
fn shuffling_loses_no_tracks() {
    let mut queue = loaded(&["a", "b", "c", "d", "e"], 0);
    queue.set_shuffled(true);
    let mut got = titles(&queue);
    got.sort();
    assert_eq!(got, tracks(&["a", "b", "c", "d", "e"]));
}

#[test]
fn setting_the_state_it_is_in_is_not_a_reshuffle() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    queue.set_shuffled(true);
    let after = titles(&queue);
    queue.set_shuffled(true);
    assert_eq!(titles(&queue), after);
}

#[test]
fn loading_with_shuffle_begins_from_the_chosen_track() {
    let mut queue = PlayQueue::new();
    queue.load(tracks(&["a", "b", "c", "d"]), 3, true);
    assert_eq!(current(&queue), Some("d"));
    assert!(queue.is_shuffled());
}

// Editing

#[test]
fn play_next_lands_after_the_current_track() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    queue.play_next(tracks(&["x"]));
    assert_eq!(titles(&queue), tracks(&["a", "x", "b", "c"]));
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn append_goes_after_the_current_track() {
    let mut queue = loaded(&["a"], 0);
    queue.append(tracks(&["z"]));
    assert_eq!(titles(&queue), tracks(&["a", "z"]));
}

#[test]
fn appending_to_an_empty_queue_makes_it_current() {
    let mut queue = PlayQueue::new();
    queue.append(tracks(&["a"]));
    assert_eq!(current(&queue), Some("a"));
}

#[test]
fn tracks_queued_while_shuffled_survive_unshuffling() {
    let mut queue = PlayQueue::new();
    queue.load(tracks(&["a", "b"]), 0, true);
    queue.append(tracks(&["z"]));
    queue.set_shuffled(false);
    assert!(titles(&queue).contains(&"z".to_owned()));
}

#[test]
fn play_next_keeps_its_place_when_shuffle_is_turned_off() {
    let mut queue = PlayQueue::new();
    queue.load(tracks(&["a", "b", "c", "d", "e"]), 0, true);
    queue.play_next(tracks(&["x"]));
    assert_eq!(queue.up_next()[0].track, "x");
    queue.set_shuffled(false);
    let at = titles(&queue).iter().position(|t| t == "x").unwrap();
    assert_eq!(
        at,
        queue.position().unwrap() + 1,
        "still the very next thing to play"
    );
}

#[test]
fn play_next_without_a_saved_order() {
    let mut queue = loaded(&["a", "b"], 0);
    queue.play_next(tracks(&["x"]));
    assert_eq!(titles(&queue), tracks(&["a", "x", "b"]));
}

#[test]
fn removing_an_earlier_track_keeps_the_same_song_playing() {
    let mut queue = loaded(&["a", "b", "c"], 2);
    let first = queue.entries()[0].id;
    queue.remove(first);
    assert_eq!(current(&queue), Some("c"));
}

#[test]
fn removing_the_current_track_moves_to_what_took_its_place() {
    let mut queue = loaded(&["a", "b", "c"], 1);
    let id = queue.entries()[1].id;
    queue.remove(id);
    assert_eq!(titles(&queue), tracks(&["a", "c"]));
    assert_eq!(current(&queue), Some("c"));
}

#[test]
fn removing_the_last_track_empties_the_queue() {
    let mut queue = loaded(&["a"], 0);
    let id = queue.entries()[0].id;
    queue.remove(id);
    assert!(queue.is_empty());
    assert_eq!(queue.position(), None);
    assert!(queue.current().is_none());
}

#[test]
fn jumping_makes_a_slot_current_and_an_unknown_id_changes_nothing() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    let id = queue.entries()[2].id;
    assert!(queue.jump(id));
    assert_eq!(current(&queue), Some("c"));
    assert!(!queue.jump(EntryId::fresh()));
    assert_eq!(current(&queue), Some("c"));
}

#[test]
fn an_out_of_range_drag_is_ignored_whole() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    let before = titles(&queue);
    queue.move_entries(&[99], 1);
    assert_eq!(titles(&queue), before);
    queue.move_entries(&[0, 42], 1);
    assert_eq!(
        titles(&queue),
        before,
        "a partly valid set is ignored, not applied in part"
    );
    queue.move_entries(&[0], 99);
    assert_eq!(titles(&queue), before);
}

#[test]
fn a_drag_on_an_empty_queue_is_nothing() {
    let mut queue: PlayQueue<String> = PlayQueue::new();
    queue.move_entries(&[0], 0);
    assert!(queue.is_empty());
}

#[test]
fn moving_to_the_very_end_is_allowed() {
    let mut queue = loaded(&["a", "b", "c"], 0);
    queue.move_entries(&[0], 3);
    assert_eq!(titles(&queue), tracks(&["b", "c", "a"]));
}

#[test]
fn reordering_does_not_change_what_is_playing() {
    let mut queue = loaded(&["a", "b", "c", "d"], 0);
    queue.move_entries(&[3], 1);
    assert_eq!(current(&queue), Some("a"));
    assert_eq!(titles(&queue), tracks(&["a", "d", "b", "c"]));
}

// Clearing up next

#[test]
fn clearing_keeps_the_current_track_and_what_came_before() {
    let mut queue = loaded(&["a", "b", "c", "d"], 1);
    queue.clear_up_next();
    assert_eq!(titles(&queue), tracks(&["a", "b"]));
    assert_eq!(current(&queue), Some("b"));
    assert!(queue.up_next().is_empty());
}

#[test]
fn clearing_under_shuffle_forgets_the_same_tracks_in_the_saved_order() {
    let mut queue = PlayQueue::new();
    queue.load(tracks(&["a", "b", "c", "d", "e"]), 0, true);
    let now = current(&queue).unwrap().to_owned();
    queue.clear_up_next();
    assert!(queue.up_next().is_empty());
    queue.set_shuffled(false);
    assert_eq!(titles(&queue), vec![now]);
}

#[test]
fn clearing_an_empty_queue_is_nothing() {
    let mut queue: PlayQueue<String> = PlayQueue::new();
    queue.clear_up_next();
    assert!(queue.is_empty());
}

// The manual lane

#[test]
fn play_last_lands_after_the_current_track_not_at_the_end_of_the_album() {
    let mut queue = loaded(&["a1", "a2", "a3", "a4"], 0);
    queue.append(tracks(&["wanted"]));
    assert_eq!(ids(queue.up_next()), ["wanted", "a2", "a3", "a4"]);
    assert_eq!(ids(queue.lane()), ["wanted"]);
    assert_eq!(ids(queue.context_up_next()), ["a2", "a3", "a4"]);
}

#[test]
fn play_next_goes_to_the_front_of_the_lane_play_last_to_the_back() {
    let mut queue = loaded(&["a1", "a2"], 0);
    queue.append(tracks(&["second"]));
    queue.append(tracks(&["third"]));
    queue.play_next(tracks(&["first"]));
    assert_eq!(ids(queue.lane()), ["first", "second", "third"]);
    assert_eq!(ids(queue.context_up_next()), ["a2"]);
}

#[test]
fn a_lane_track_that_has_played_leaves_the_lane() {
    let mut queue = loaded(&["a1", "a2"], 0);
    queue.append(tracks(&["q1", "q2"]));
    assert_eq!(queue.lane().len(), 2);
    queue.skip_to_next();
    assert_eq!(current(&queue), Some("q1"));
    assert_eq!(ids(queue.lane()), ["q2"]);
    queue.skip_to_next();
    queue.skip_to_next();
    assert_eq!(current(&queue), Some("a2"));
    assert!(queue.lane().is_empty());
}

#[test]
fn shuffle_leaves_the_lane_in_place() {
    let mut queue = loaded(&["a1", "a2", "a3", "a4", "a5", "a6"], 0);
    queue.append(tracks(&["q1", "q2"]));
    queue.set_shuffled(true);
    assert_eq!(current(&queue), Some("a1"));
    assert_eq!(ids(queue.lane()), ["q1", "q2"]);
    let mut rest = ids(queue.context_up_next());
    rest.sort();
    assert_eq!(rest, ["a2", "a3", "a4", "a5", "a6"]);
}

#[test]
fn starting_something_else_keeps_what_was_queued_by_hand() {
    let mut queue = loaded(&["a1", "a2"], 0);
    queue.append(tracks(&["kept"]));
    queue.load(tracks(&["b1", "b2", "b3"]), 0, false);
    assert_eq!(current(&queue), Some("b1"));
    assert_eq!(ids(queue.lane()), ["kept"]);
    assert_eq!(ids(queue.context_up_next()), ["b2", "b3"]);
}

#[test]
fn a_lane_track_already_played_is_not_carried_over() {
    let mut queue = loaded(&["a1", "a2"], 0);
    queue.append(tracks(&["q1"]));
    queue.skip_to_next();
    queue.load(tracks(&["b1", "b2"]), 0, false);
    assert!(queue.lane().is_empty());
    assert_eq!(titles(&queue), tracks(&["b1", "b2"]));
}

#[test]
fn album_tracks_are_not_manual() {
    let queue = loaded(&["a1", "a2"], 0);
    assert!(queue.entries().iter().all(|e| !e.is_manual));
    assert!(queue.lane().is_empty());
}

// Extending a running queue

#[test]
fn found_songs_go_behind_everything_including_the_lane() {
    let mut queue = loaded(&["1", "2"], 0);
    queue.play_next(tracks(&["hand"]));
    queue.extend(tracks(&["found"]));
    assert_eq!(titles(&queue), tracks(&["1", "hand", "2", "found"]));
    assert!(!queue.entries().last().unwrap().is_manual);
    assert_eq!(ids(queue.lane()), ["hand"]);
}

#[test]
fn found_songs_do_not_move_the_listener() {
    let mut queue = loaded(&["1", "2"], 1);
    queue.extend(tracks(&["found"]));
    assert_eq!(current(&queue), Some("2"));
}

#[test]
fn found_songs_shuffle_like_context() {
    let names: Vec<String> = (1..=8).map(|n| n.to_string()).collect();
    let mut queue = PlayQueue::new();
    queue.load(names.clone(), 0, false);
    queue.set_shuffled(true);
    queue.extend(tracks(&["found"]));
    assert!(titles(&queue).contains(&"found".to_owned()));
    queue.set_shuffled(false);
    let mut want = names;
    want.push("found".into());
    assert_eq!(titles(&queue), want);
}

#[test]
fn nothing_to_add_changes_nothing() {
    let mut queue = loaded(&["1"], 0);
    queue.extend(Vec::new());
    assert_eq!(queue.entries().len(), 1);
}

// The render window (RenderWindowTests in the iOS app)

#[test]
fn the_render_window_follows_repeat_one_with_separate_occurrences() {
    let mut queue = loaded(&["a", "b"], 0);
    queue.repeat_mode = RepeatMode::One;
    let window = queue.render_window(3);
    assert_eq!(
        window.iter().map(|s| s.track.as_str()).collect::<Vec<_>>(),
        ["a", "a", "a"]
    );
    assert_eq!(
        window.iter().map(|s| s.occurrence).collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn the_render_window_stops_at_the_end_and_wraps_under_repeat_all() {
    let queue = loaded(&["a", "b"], 0);
    assert_eq!(queue.render_window(5).len(), 2);
    let mut queue = loaded(&["a", "b"], 1);
    queue.repeat_mode = RepeatMode::All;
    assert_eq!(
        queue
            .render_window(3)
            .iter()
            .map(|s| s.track.as_str())
            .collect::<Vec<_>>(),
        ["b", "a", "b"]
    );
}
