//! The playlist page's reading of the store, and its moves.

use aoide_store::PlaySource;

use super::*;

#[test]
fn a_smart_playlist_is_evaluated_from_the_history_on_open() {
    let store = aoide_store::Store::in_memory("desk").unwrap();
    let now = 1_792_540_800_000;
    let created = store.ensure_built_in_playlists("7", now).unwrap();
    assert!(!created.is_empty());
    let heavy = aoide_store::BuiltInPlaylist::HeavyRotation;
    // Played often, lately: Heavy Rotation's kind of song.
    let track = aoide_store::TrackRef::new("t1", "k1", "Lantern", "Mara Vey", "Saltwater", now);
    store.cache_tracks(&[track]).unwrap();
    let link = aoide_store::TrackLink::new("t1", "k1");
    for n in 0..6 {
        let at = now - (n + 1) * 3_600_000;
        store
            .record_play(
                &link,
                at,
                180_000,
                Some(180_000),
                PlaySource::Album,
                Some(true),
            )
            .unwrap();
    }
    let (playlist, items, smart) = read_rows(&store, &heavy.id_for("7"), now).unwrap();
    assert!(playlist.unwrap().is_smart);
    assert!(items.is_empty(), "rules, not entries");
    assert_eq!(smart.unwrap().unwrap(), ["t1"]);
    let plain = store.create_playlist("Mine", None, None, now).unwrap();
    assert!(read_rows(&store, &plain.id, now).unwrap().2.is_none());
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn a_step_lands_beside_its_listed_neighbour_past_missing_entries() {
    // "x" is an entry the server no longer has; it is not listed.
    let all = ids(&["a", "x", "b", "c"]);
    assert_eq!(move_index(&all, "b", "a", true), Some(0));
    assert_eq!(move_index(&all, "a", "b", false), Some(2));
    assert_eq!(move_index(&all, "c", "b", true), Some(2));
    assert_eq!(move_index(&all, "b", "c", false), Some(3));
    assert_eq!(move_index(&all, "b", "gone", true), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_writes_one_entry_and_lands_where_shown() {
    let store = crate::store_worker::StoreHandle::in_memory("device-1").unwrap();
    store
        .call(|w| {
            let list = w.store.create_playlist("Set", None, None, 1).unwrap();
            let link = |id: &str| aoide_store::TrackLink::new(id.to_owned(), format!("k-{id}"));
            let added = w
                .store
                .add_tracks(&["a", "x", "b", "c"].map(link), &list.id, 1)
                .unwrap();
            let all: Vec<String> = added.iter().map(|i| i.id.clone()).collect();
            let before = w.store.pending_op_count().unwrap();
            // "c" steps up past "b"; "x" (not listed) stays between a and b.
            let index = move_index(&all, &all[3], &all[2], true).unwrap();
            w.store.move_item(&all[3], &list.id, index, 2).unwrap();
            assert_eq!(w.store.pending_op_count().unwrap(), before + 1);
            let order: Vec<String> = w
                .store
                .items(&list.id)
                .unwrap()
                .into_iter()
                .map(|i| i.jellyfin_id)
                .collect();
            assert_eq!(order, ["a", "x", "c", "b"]);
        })
        .await
        .unwrap();
}
