//! Port of `TrackFlagsTests.swift`, suite "Taste flags". The "Holding entities the
//! server does not accept" suite exercises the engine and lives in `aoide-sidecar`.

use std::collections::BTreeSet;

use super::{count, pending, store};
use crate::{Calendar, OpKind, Record, SmartRules, Store, TrackFlags, TrackRef, now_millis};

fn cache(store: &Store, ids: &[&str]) {
    store
        .write_local(|tx| {
            for id in ids {
                TrackRef::new(*id, format!("key-{id}"), *id, "a", "b", 0).insert(tx)?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn setting_a_flag_mints_one_row_per_track_and_records_an_op() {
    let store = store();
    store.set_not_interested(true, "t1", "k1", 10).unwrap();
    store.set_dont_count(true, "t1", "k1", 11).unwrap();
    let flags = store.flags("t1").unwrap().unwrap();
    assert!(flags.not_interested && flags.dont_count);
    assert_eq!(count::<TrackFlags>(&store), 1);
    let ops: Vec<_> = pending(&store)
        .into_iter()
        .filter(|op| op.entity == "trackFlags")
        .collect();
    assert_eq!(ops.len(), 2);
    assert!(ops.iter().all(|op| op.entity_id == flags.id));
}

#[test]
fn clearing_both_flags_deletes_the_row_rather_than_keeping_an_empty_one() {
    let store = store();
    store.set_not_interested(true, "t1", "k1", 10).unwrap();
    store.set_not_interested(false, "t1", "k1", 20).unwrap();
    assert_eq!(store.flags("t1").unwrap(), None);
    assert_eq!(
        pending(&store).last().unwrap().operation,
        OpKind::Delete.as_str()
    );
}

#[test]
fn a_flag_set_again_after_deletion_reuses_the_rows_id() {
    let store = store();
    store.set_not_interested(true, "t1", "k1", 10).unwrap();
    let first = store.flags("t1").unwrap().unwrap().id;
    store.set_not_interested(false, "t1", "k1", 20).unwrap();
    store.set_dont_count(true, "t1", "k1", 30).unwrap();
    assert_eq!(store.flags("t1").unwrap().unwrap().id, first);
}

#[test]
fn not_interested_tracks_fall_out_of_every_smart_playlist() {
    let store = store();
    cache(&store, &["a", "b", "c"]);
    store.set_not_interested(true, "b", "key-b", 10).unwrap();
    let everything = SmartRules::everything();
    let evaluated = |store: &Store| -> BTreeSet<String> {
        store
            .evaluate(&everything, now_millis(), Calendar::Local)
            .unwrap()
            .into_iter()
            .collect()
    };
    assert_eq!(evaluated(&store), ["a".to_owned(), "c".to_owned()].into());
    store.set_dont_count(true, "c", "key-c", 11).unwrap();
    // dontCount alone does not hide a track.
    assert_eq!(evaluated(&store), ["a".to_owned(), "c".to_owned()].into());
}

#[test]
fn not_interested_among_answers_for_a_list_in_chunks() {
    let store = store();
    let ids: Vec<String> = (0..900).map(|i| format!("t{i}")).collect();
    store.set_not_interested(true, "t5", "k", 1).unwrap();
    store.set_not_interested(true, "t850", "k", 1).unwrap();
    assert_eq!(
        store.not_interested(&ids).unwrap(),
        ["t5".to_owned(), "t850".to_owned()].into()
    );
    assert!(store.not_interested(&[]).unwrap().is_empty());
}

#[test]
fn flagged_tracks_lists_them_newest_first_with_what_the_cache_knows() {
    let store = store();
    cache(&store, &["a"]);
    store.set_dont_count(true, "a", "key-a", 1).unwrap();
    store.set_not_interested(true, "zz", "key-zz", 2).unwrap();
    let listed = store.flagged_tracks().unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|(f, _)| f.jellyfin_id.as_str())
            .collect::<Vec<_>>(),
        ["zz", "a"]
    );
    assert_eq!(listed[1].1.as_ref().map(|t| t.title.as_str()), Some("a"));
    assert!(listed[0].1.is_none());
}
