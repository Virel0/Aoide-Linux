//! Waiting songs a build before 0.5.0 quarantined, put back by the replay at the
//! start of each sync. The setup is what that build left: the probe playlist's
//! ordinary ops applied, its two waiting songs in quarantine.

use serde_json::json;

use super::{all, fetch, store};
use crate::quarantine::{QuarantinedOp, SyncDirection};
use crate::record::Record;
use crate::{PlaylistItem, PullResponse, Store, SyncPolicy};

const ON_ITS_WAY: &str = "F1E0C716-429E-4BA5-B34A-0B234D2DB064";
const NOT_FOUND: &str = "517159EF-7350-4993-AEB5-913FA05C431F";
/// The op Melete wrote for `ON_ITS_WAY`.
const ON_ITS_WAY_OP: &str = "C01FDB2D-8156-4C4A-9259-E99DD13FBE58";
const NOW: i64 = 1_793_671_990_000;
/// When the older build set them aside.
const SET_ASIDE: i64 = 1_793_671_989_500;

/// The store an older build left: ordinary ops applied, waiting songs held.
fn left_by_an_older_build() -> Store {
    let store = store();
    let page: PullResponse =
        serde_json::from_str(include_str!("pending_probe.json")).expect("fixture");
    let (waiting, ordinary): (Vec<_>, Vec<_>) = page
        .ops
        .into_iter()
        .partition(|op| op.payload.get("pending").is_some());
    store
        .apply(&ordinary, page.cursor, SyncPolicy::default(), NOW)
        .unwrap();
    store
        .write_local(|tx| {
            for op in &waiting {
                QuarantinedOp {
                    op_id: op.op_id.clone(),
                    entity: op.entity.clone(),
                    entity_id: op.entity_id.clone(),
                    direction: SyncDirection::Inbound.as_str().to_owned(),
                    reason: "missing field `jellyfinId`".into(),
                    payload: op.payload.to_string(),
                    quarantined_at: SET_ASIDE,
                    operation: None,
                    created_at: None,
                }
                .save(tx)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

fn replay(store: &Store) -> crate::SyncReport {
    store
        .replay_quarantined_waiting_songs(SyncPolicy::default(), NOW)
        .unwrap()
}

#[test]
fn waiting_songs_an_older_build_set_aside_come_back_in_place() {
    let store = left_by_an_older_build();
    assert_eq!(all::<QuarantinedOp>(&store).len(), 2);

    let report = replay(&store);
    assert_eq!((report.applied, report.quarantined_inbound), (2, 0));
    assert!(all::<QuarantinedOp>(&store).is_empty());

    let on_its_way: PlaylistItem = fetch(&store, ON_ITS_WAY).unwrap();
    assert_eq!(on_its_way.pending.unwrap().request_id, Some(5));
    assert_eq!(on_its_way.position, "W");
    assert!(
        fetch::<PlaylistItem>(&store, NOT_FOUND)
            .unwrap()
            .is_waiting()
    );
}

#[test]
fn a_track_swapped_in_since_wins_and_the_held_copy_still_leaves() {
    let store = left_by_an_older_build();
    let held = fetch::<QuarantinedOp>(&store, ON_ITS_WAY_OP).unwrap();
    let mut fields: serde_json::Value = serde_json::from_str(&held.payload).unwrap();
    let object = fields.as_object_mut().unwrap();
    object.remove("pending");
    object.insert("jellyfinId".into(), json!("9"));
    object.insert(
        "contentKey".into(),
        json!("radiohead|ok computer|no surprises|228"),
    );
    object.insert("updatedAt".into(), json!(SET_ASIDE + 100));
    let item: PlaylistItem = serde_json::from_value(fields).unwrap();
    store.read(|conn| item.save(conn)).unwrap();

    replay(&store);
    let kept: PlaylistItem = fetch(&store, ON_ITS_WAY).unwrap();
    assert!(!kept.is_waiting());
    assert_eq!(kept.jellyfin_id, "9");
    assert!(
        all::<QuarantinedOp>(&store).is_empty(),
        "both left quarantine"
    );
}

#[test]
fn other_quarantined_ops_stay_where_they_are() {
    let store = left_by_an_older_build();
    store
        .write_local(|tx| {
            QuarantinedOp {
                op_id: "UNKNOWN".into(),
                entity: "stems".into(),
                entity_id: "s1".into(),
                direction: SyncDirection::Inbound.as_str().to_owned(),
                reason: "unknown entity".into(),
                payload: json!({"id": "s1", "pending": {}}).to_string(),
                quarantined_at: SET_ASIDE,
                operation: None,
                created_at: None,
            }
            .save(tx)?;
            Ok(())
        })
        .unwrap();

    replay(&store);
    let left: Vec<String> = all::<QuarantinedOp>(&store)
        .into_iter()
        .map(|q| q.op_id)
        .collect();
    assert_eq!(left, ["UNKNOWN"]);
}

#[test]
fn replaying_again_changes_nothing() {
    let store = left_by_an_older_build();
    replay(&store);
    let report = replay(&store);
    assert_eq!((report.applied, report.quarantined_inbound), (0, 0));
    assert_eq!(all::<PlaylistItem>(&store).len(), 3);
}
