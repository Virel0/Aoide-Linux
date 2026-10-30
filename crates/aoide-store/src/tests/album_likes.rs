//! Album likes: pulled and merged as a track's like is, in a table of their own.

use serde_json::json;

use super::{Pulled, all, apply_one, pulled_raw, store};
use crate::record::Record;
use crate::{AlbumLike, Like, OpKind, SyncEntity};

fn album_like(id: &str, album: &str, updated_at: i64, device: &str) -> AlbumLike {
    AlbumLike {
        id: id.into(),
        album_id: album.into(),
        content_key: "bjork|post".into(),
        liked: true,
        updated_at,
        deleted: false,
        origin_device: device.into(),
    }
}

#[test]
fn the_entity_is_named_as_the_server_names_it() {
    assert_eq!(SyncEntity::AlbumLike.wire_name(), "album_likes");
    assert_eq!(SyncEntity::AlbumLike.local_table(), AlbumLike::TABLE);
    // Never written here, so never held: a server without album likes isn't a
    // reason to report anything waiting.
    assert!(!SyncEntity::AlbumLike.needs_server_support());
}

#[test]
fn a_pulled_album_like_is_kept_and_likes_no_track() {
    let store = store();
    let row = album_like("A", "1", 2_000, "web");
    let report = apply_one(&store, Pulled::new(SyncEntity::AlbumLike, &row, "A").op());
    assert_eq!((report.applied, report.quarantined_inbound), (1, 0));
    assert_eq!(all::<AlbumLike>(&store), [row]);
    assert!(all::<Like>(&store).is_empty());
}

/// As with a track, two devices liking one album offline mint two rows; the later wins.
#[test]
fn two_likes_of_one_album_collapse_to_the_later() {
    let store = store();
    apply_one(
        &store,
        Pulled::new(
            SyncEntity::AlbumLike,
            &album_like("A", "1", 1_000, "phone"),
            "A",
        )
        .op(),
    );
    let later = album_like("B", "1", 3_000, "web");
    let report = apply_one(&store, Pulled::new(SyncEntity::AlbumLike, &later, "B").op());
    assert_eq!(report.superseded_likes, 1);
    assert_eq!(all::<AlbumLike>(&store), [later]);
}

#[test]
fn a_withdrawn_album_like_is_a_tombstone() {
    let store = store();
    apply_one(
        &store,
        Pulled::new(
            SyncEntity::AlbumLike,
            &album_like("A", "1", 1_000, "web"),
            "A",
        )
        .op(),
    );
    let gone = AlbumLike {
        deleted: true,
        ..album_like("A", "1", 2_000, "web")
    };
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::AlbumLike, &gone, "A")
            .operation(OpKind::Delete)
            .op(),
    );
    assert_eq!(report.applied, 1);
    assert!(all::<AlbumLike>(&store)[0].deleted);
}

/// A row without `albumId` (a track's like sent under the wrong entity) can't be read.
#[test]
fn a_track_row_under_the_album_entity_is_quarantined() {
    let store = store();
    let payload = json!({"id": "T", "jellyfinId": "7", "contentKey": "k", "liked": true,
        "updatedAt": 1, "deleted": false, "originDevice": "web"});
    let report = apply_one(&store, pulled_raw("album_likes", "T", payload, "upsert", 1));
    assert_eq!(report.quarantined_inbound, 1);
    assert!(all::<AlbumLike>(&store).is_empty());
}
