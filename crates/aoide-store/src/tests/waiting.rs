//! Entries for songs the library does not have yet, against the ops Melete 0.4.0
//! wrote for a playlist with one track, one song on its way and one it could not
//! ask for (`pending_probe.json`, pulled from a test server).

use serde_json::{Value, json};

use super::{all, fetch, pending, store};
use crate::record::Record;
use crate::{
    PendingSong, PlaylistCover, PlaylistItem, PullResponse, Store, SyncPolicy, SyncReport,
};

const PLAYLIST: &str = "0B8A1E8B-B3DF-45A7-88E0-EA53F78393C3";
const TRACK: &str = "4DDD98DA-9A44-4EB7-AFF4-89EEF8134A74";
const ON_ITS_WAY: &str = "F1E0C716-429E-4BA5-B34A-0B234D2DB064";
const NOT_FOUND: &str = "517159EF-7350-4993-AEB5-913FA05C431F";
/// Just after the last op was received.
const NOW: i64 = 1_793_671_990_000;

fn probe() -> PullResponse {
    serde_json::from_str(include_str!("pending_probe.json")).expect("fixture")
}

fn pull(store: &Store, page: &PullResponse) -> SyncReport {
    store
        .apply(&page.ops, page.cursor, SyncPolicy::default(), NOW)
        .expect("apply")
}

#[test]
fn melete_s_waiting_entries_are_kept_in_place_and_nothing_is_quarantined() {
    let store = store();
    let report = pull(&store, &probe());
    assert_eq!((report.applied, report.quarantined_inbound), (4, 0));

    let items = store.items(PLAYLIST).unwrap();
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, [TRACK, ON_ITS_WAY, NOT_FOUND]);
    assert!(!items[0].is_waiting());
    assert_eq!(items[0].jellyfin_id, "2");

    let song = items[1].pending.as_ref().unwrap();
    assert!(items[1].is_waiting());
    assert_eq!(items[1].jellyfin_id, "");
    assert_eq!(song.request_id, Some(5));
    assert_eq!(song.title, "No Surprises");
    assert_eq!(song.artist.as_deref(), Some("Radiohead"));
    assert_eq!(song.album.as_deref(), Some("OK Computer"));

    let lost = items[2].pending.as_ref().unwrap();
    assert_eq!(lost.request_id, None);
    assert!(lost.why.as_deref().unwrap().contains("MusicBrainz"));
}

#[test]
fn both_shapes_round_trip_through_the_payload_unchanged() {
    for op in probe().ops {
        if op.entity != "playlist_items" {
            continue;
        }
        let item: PlaylistItem = serde_json::from_value(op.payload.clone()).unwrap();
        assert_eq!(serde_json::to_value(&item).unwrap(), op.payload);
    }
}

#[test]
fn both_shapes_round_trip_through_the_database() {
    let store = store();
    let page = probe();
    pull(&store, &page);
    for op in page.ops.iter().filter(|op| op.entity == "playlist_items") {
        let stored: PlaylistItem = fetch(&store, &op.entity_id).unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap(), op.payload);
    }
    assert_eq!(all::<PlaylistItem>(&store).len(), 3);
}

/// What Melete writes once the song is in the library: the same id, playlist and
/// position, the track in place of `pending`, from its own device, a little later.
fn arrived(payload: &Value, updated_at: i64) -> Value {
    let mut fields = payload.clone();
    let object = fields.as_object_mut().unwrap();
    object.remove("pending");
    object.insert("jellyfinId".into(), json!("9"));
    object.insert(
        "contentKey".into(),
        json!("radiohead|ok computer|no surprises|228"),
    );
    object.insert("updatedAt".into(), json!(updated_at));
    fields
}

#[test]
fn a_waiting_entry_upserted_with_its_track_becomes_a_normal_one() {
    let store = store();
    let mut page = probe();
    pull(&store, &page);

    let waiting = page
        .ops
        .iter()
        .find(|op| op.entity_id == ON_ITS_WAY)
        .unwrap();
    let mut swap = waiting.clone();
    swap.op_id = "SWAP".into();
    swap.seq += 10;
    swap.payload = arrived(&waiting.payload, 1_793_671_986_056);
    swap.received_at = 1_793_671_989_000;
    page.ops = vec![swap];
    page.cursor = crate::PullCursor(116);
    let report = pull(&store, &page);
    assert_eq!((report.applied, report.quarantined_inbound), (1, 0));

    let item: PlaylistItem = fetch(&store, ON_ITS_WAY).unwrap();
    assert!(!item.is_waiting());
    assert_eq!(item.pending, None);
    assert_eq!(item.jellyfin_id, "9");
    assert_eq!(item.position, "W");
    assert_eq!(item.origin_device, "melete");
}

#[test]
fn an_older_swap_loses_to_an_edit_made_since() {
    let store = store();
    let mut page = probe();
    pull(&store, &page);
    let mut removed: PlaylistItem = fetch(&store, ON_ITS_WAY).unwrap();
    removed.deleted = true;
    removed.updated_at = 1_793_671_987_000;
    store.read(|conn| removed.save(conn)).unwrap();

    let waiting = page
        .ops
        .iter()
        .find(|op| op.entity_id == ON_ITS_WAY)
        .unwrap();
    let mut swap = waiting.clone();
    swap.op_id = "SWAP".into();
    swap.payload = arrived(&waiting.payload, 1_793_671_986_056);
    page.ops = vec![swap];
    assert_eq!(pull(&store, &page).ignored_stale, 1);
    assert!(fetch::<PlaylistItem>(&store, ON_ITS_WAY).unwrap().deleted);
}

#[test]
fn an_entry_with_neither_a_track_nor_a_song_is_quarantined() {
    let store = store();
    let mut page = probe();
    let op = page
        .ops
        .iter_mut()
        .find(|op| op.entity_id == ON_ITS_WAY)
        .unwrap();
    op.payload.as_object_mut().unwrap().remove("pending");
    let report = pull(&store, &page);
    assert_eq!((report.applied, report.quarantined_inbound), (3, 1));
}

#[test]
fn a_song_added_here_goes_out_without_a_track_and_after_the_rest() {
    let store = store();
    let list = store.create_playlist("Soon", None, None, 1).unwrap();
    store
        .add_tracks(&super::links(&["a"]), &list.id, 1)
        .unwrap();
    let song = PendingSong::requested(5, "No Surprises", Some("Radiohead".into()));
    let added = store
        .add_waiting(std::slice::from_ref(&song), &list.id, 2)
        .unwrap();

    let items = store.items(&list.id).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].id, added[0].id);
    assert_eq!(items[1].pending.as_ref(), Some(&song));

    let op = pending(&store).pop().unwrap();
    let payload: Value = serde_json::from_str(&op.payload).unwrap();
    assert!(payload.get("jellyfinId").is_none());
    assert!(payload.get("contentKey").is_none());
    assert_eq!(
        payload["pending"],
        json!({"album": null, "artist": "Radiohead", "durationMs": null,
               "requestId": 5, "title": "No Surprises", "why": null})
    );
}

#[test]
fn relinking_passes_waiting_entries_by() {
    let store = store();
    pull(&store, &probe());
    let report = store.relink_items(|_| false, |_| None, NOW).unwrap();
    assert_eq!((report.checked, report.unresolved), (1, 1));
    let waiting: PlaylistItem = fetch(&store, ON_ITS_WAY).unwrap();
    assert!(waiting.is_waiting());
    assert!(!waiting.is_unresolved());
}

#[test]
fn a_cover_is_never_drawn_from_a_waiting_entry() {
    let store = store();
    let list = store.create_playlist("Soon", None, None, 1).unwrap();
    let song = PendingSong::requested(5, "No Surprises", None);
    store.add_waiting(&[song], &list.id, 1).unwrap();
    let row = store.playlist(&list.id).unwrap().unwrap();
    assert_eq!(store.playlist_cover(&row).unwrap(), PlaylistCover::None);

    store
        .add_tracks(&super::links(&["t1"]), &list.id, 2)
        .unwrap();
    assert_eq!(
        store.playlist_cover(&row).unwrap(),
        PlaylistCover::FirstTrack("t1".into())
    );
    let albums = store
        .cover_albums(std::slice::from_ref(&list.id), 4)
        .unwrap();
    assert!(albums.get(&list.id).is_none_or(Vec::is_empty));
}

/// A database from before waiting entries, with an entry in it, opens and reads.
#[test]
fn an_older_database_upgrades_and_keeps_its_entries() {
    let path = std::env::temp_dir().join(format!("aoide-v10-{}.sqlite", crate::new_id()));
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"CREATE TABLE "schemaMigration" ("identifier" TEXT PRIMARY KEY NOT NULL)"#,
        )
        .unwrap();
        for (identifier, sql) in crate::schema::MIGRATIONS.iter().take(10) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                r#"INSERT INTO "schemaMigration" ("identifier") VALUES (?)"#,
                [identifier],
            )
            .unwrap();
        }
        conn.execute(
            r#"INSERT INTO "playlistItem" VALUES ('i1', 'p1', 't1', 'k1', 'V', 1, 0, 'mac')"#,
            [],
        )
        .unwrap();
    }
    let store = Store::open(&path, "desk").unwrap();
    let items = store.items("p1").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].jellyfin_id.as_str(), items[0].pending.as_ref()),
        ("t1", None)
    );
    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
