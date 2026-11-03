//! Songs waiting in playlists, through the whole engine: the entries Melete 0.4.0
//! wrote for a test playlist (one track, one song on its way, one it could not ask
//! for), its later swap of the track in, and a waiting entry made on one device
//! arriving on another.

use std::sync::Arc;

use aoide_store::{Millis, PendingSong, PullResponse, Store, SyncPolicy, WireOp};
use serde_json::json;

use super::sidecar_stub::{Sidecar, SidecarLink};
use super::store;
use crate::SyncEngine;

const PLAYLIST: &str = "0B8A1E8B-B3DF-45A7-88E0-EA53F78393C3";
const ON_ITS_WAY: &str = "F1E0C716-429E-4BA5-B34A-0B234D2DB064";
/// Just after Melete wrote the entries.
const CLOCK: Millis = 1_793_671_990_000;

fn engine(store: &Arc<Store>, link: SidecarLink) -> SyncEngine<SidecarLink> {
    SyncEngine::new(Arc::clone(store), link)
        .with_policy(SyncPolicy::default())
        .with_clock(|| CLOCK)
}

/// The test server's ops for the playlist, as Melete's device wrote them.
fn melete_ops() -> Vec<WireOp> {
    let page: PullResponse = serde_json::from_str(include_str!(
        "../../../aoide-store/src/tests/pending_probe.json"
    ))
    .expect("fixture");
    page.ops
        .into_iter()
        .map(|op| WireOp {
            op_id: op.op_id,
            entity: op.entity,
            entity_id: op.entity_id,
            operation: op.operation,
            payload: op.payload,
            created_at: op.created_at,
        })
        .collect()
}

#[tokio::test]
async fn waiting_entries_sync_in_and_turn_into_tracks_when_melete_swaps_them() {
    let sidecar = Sidecar::with_clock(CLOCK);
    let ops = melete_ops();
    let swap_base = ops
        .iter()
        .find(|op| op.entity_id == ON_ITS_WAY)
        .cloned()
        .unwrap();
    sidecar.seed("listener", ops, "melete");
    let desk = store("desk");

    let first = engine(&desk, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!((first.applied, first.quarantined_inbound), (4, 0));
    let items = desk.items(PLAYLIST).unwrap();
    let waiting: Vec<bool> = items.iter().map(|i| i.is_waiting()).collect();
    assert_eq!(waiting, [false, true, true]);

    let mut swap = swap_base;
    swap.op_id = "SWAP-1".into();
    let fields = swap.payload.as_object_mut().unwrap();
    fields.remove("pending");
    fields.insert("jellyfinId".into(), json!("9"));
    fields.insert(
        "contentKey".into(),
        json!("radiohead|ok computer|no surprises|228"),
    );
    fields.insert("updatedAt".into(), json!(1_793_671_986_056_i64));
    sidecar.seed("listener", vec![swap], "melete");

    let second = engine(&desk, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!((second.applied, second.quarantined_inbound), (1, 0));
    let items = desk.items(PLAYLIST).unwrap();
    assert_eq!(items.len(), 3);
    assert!(!items[1].is_waiting());
    assert_eq!(
        (items[1].id.as_str(), items[1].jellyfin_id.as_str()),
        (ON_ITS_WAY, "9")
    );
    assert!(items[2].is_waiting(), "the one never asked for waits on");
}

#[tokio::test]
async fn a_waiting_entry_made_here_reaches_another_device_without_a_track() {
    let sidecar = Sidecar::with_clock(CLOCK);
    let desk = store("desk");
    let laptop = store("laptop");
    let list = desk
        .create_playlist("Soon", None, None, CLOCK - 10)
        .unwrap();
    let song = PendingSong::requested(5, "No Surprises", Some("Radiohead".into()));
    desk.add_waiting(std::slice::from_ref(&song), &list.id, CLOCK - 5)
        .unwrap();

    let sent = engine(&desk, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!((sent.pushed, sent.rejected), (2, 0));
    let pushed = sidecar
        .ops_for("listener")
        .into_iter()
        .find(|row| row.op.entity == "playlist_items")
        .unwrap();
    assert!(pushed.op.payload.get("jellyfinId").is_none());
    assert_eq!(pushed.op.payload["pending"]["requestId"], json!(5));

    let received = engine(&laptop, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(received.quarantined_inbound, 0);
    let items = laptop.items(&list.id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].pending.as_ref(), Some(&song));
}
