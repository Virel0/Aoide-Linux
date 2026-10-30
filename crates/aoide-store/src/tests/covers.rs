//! Covers: the order a cover is chosen in, and a picture from a file.

use super::{links, pending, store};
use crate::{
    BlobDirectory, CoverError, MAX_IMAGE_BYTES, Playlist, PlaylistCover, Store, StoreError,
    TrackRef, is_image_hash, new_id, now_millis, sniff_image_mime,
};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 9, 9];

fn blobs() -> BlobDirectory {
    BlobDirectory::open(std::env::temp_dir().join(format!("aoide-blobs-{}", new_id()))).unwrap()
}

fn cache(store: &Store, track: &str, album: Option<&str>) {
    let mut row = TrackRef::new(track, format!("key-{track}"), track, "Artist", "Album", 1);
    row.album_id = album.map(str::to_owned);
    store.cache_tracks(&[row]).unwrap();
}

fn cover(store: &Store, id: &str) -> PlaylistCover {
    store
        .playlist_cover(&store.playlist(id).unwrap().unwrap())
        .unwrap()
}

// MARK: - Which picture

#[test]
fn a_chosen_picture_beats_a_chosen_album_beats_the_first_track() {
    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Covered", None, None, now).unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::None,
        "empty: the lettered tile"
    );

    cache(&store, "t1", Some("album-1"));
    store
        .add_tracks(&links(&["t1", "t2"]), &list.id, now)
        .unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::FirstTrack("album-1".into())
    );

    let mut row = store.playlist(&list.id).unwrap().unwrap();
    row.source_jellyfin_id = Some("jf-playlist".into());
    store.record(row, false, now).unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::FirstTrack("album-1".into()),
        "the Jellyfin playlist it came from is no cover"
    );

    store.set_artwork(Some("album-9"), &list.id, now).unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::Album("album-9".into())
    );

    let blobs = blobs();
    let saved = store.set_cover_image(&list.id, PNG, &blobs, now).unwrap();
    let hash = saved.image_hash.clone().unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::Image {
            hash,
            mime: Some("image/png".into())
        }
    );
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

/// A track the cache has never seen still has a picture of its own.
#[test]
fn the_first_track_stands_in_by_its_own_id_when_its_album_is_unknown() {
    let store = store();
    let list = store
        .create_playlist("Bare", None, None, now_millis())
        .unwrap();
    store
        .add_tracks(&links(&["t-unknown"]), &list.id, now_millis())
        .unwrap();
    assert_eq!(
        cover(&store, &list.id),
        PlaylistCover::FirstTrack("t-unknown".into())
    );
}

#[test]
fn covers_for_a_whole_screen_come_from_one_call() {
    let store = store();
    let now = now_millis();
    let a = store.create_playlist("A", None, None, now).unwrap();
    let b = store.create_playlist("B", None, None, now).unwrap();
    cache(&store, "t1", Some("album-1"));
    store.add_tracks(&links(&["t1"]), &a.id, now).unwrap();
    let covers = store.playlist_covers(&store.playlists().unwrap()).unwrap();
    assert_eq!(covers[&a.id], PlaylistCover::FirstTrack("album-1".into()));
    assert_eq!(covers[&b.id], PlaylistCover::None);
}

/// Hashes arrive in synced payloads; one that is not a hash is never a file name.
#[test]
fn a_hash_that_is_not_a_hash_is_never_drawn_or_opened() {
    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Odd", None, None, now).unwrap();
    let mut row: Playlist = store.playlist(&list.id).unwrap().unwrap();
    row.image_hash = Some("../../etc/passwd".into());
    store.record(row, false, now).unwrap();
    assert_eq!(cover(&store, &list.id), PlaylistCover::None);

    let blobs = blobs();
    assert_eq!(blobs.read("../../etc/passwd"), None);
    assert!(!blobs.contains("../x"));
    assert!(is_image_hash(&Store::image_hash(b"x")));
    assert!(!is_image_hash(&Store::image_hash(b"x").to_uppercase()));
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

// MARK: - A picture from a file

#[test]
fn a_picture_is_kept_by_hash_registered_for_upload_and_named_on_the_row() {
    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Mine", None, None, now).unwrap();
    store.set_artwork(Some("album-1"), &list.id, now).unwrap();
    let blobs = blobs();
    let file = blobs.root().join("picked.png");
    std::fs::write(&file, PNG).unwrap();

    let saved = store
        .set_cover_from_file(&list.id, &file, &blobs, now)
        .unwrap();
    let hash = Store::image_hash(PNG);
    assert_eq!(saved.image_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(saved.image_mime.as_deref(), Some("image/png"));
    assert_eq!(
        saved.artwork_item_id, None,
        "a picture outranks a chosen item"
    );
    assert_eq!(blobs.read(&hash).as_deref(), Some(PNG));

    // Waiting for upload, and named by the op that must wait for it.
    let needed = store.images_needed_before_push().unwrap();
    assert_eq!(needed.len(), 1);
    assert_eq!(needed[0].hash, hash);
    assert_eq!(needed[0].mime, "image/png");
    let op = pending(&store)
        .into_iter()
        .rev()
        .find(|op| op.entity_id == list.id)
        .unwrap();
    assert!(op.payload.contains(&hash));
    assert!(!op.payload.contains("iVBOR"), "no bytes in the op");
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

#[test]
fn only_jpeg_png_and_webp_under_the_limit_are_covers() {
    assert_eq!(sniff_image_mime(JPEG), Some("image/jpeg"));
    assert_eq!(sniff_image_mime(PNG), Some("image/png"));
    assert_eq!(
        sniff_image_mime(b"RIFF\x10\0\0\0WEBPVP8 "),
        Some("image/webp")
    );
    assert_eq!(sniff_image_mime(b"GIF89a"), None);
    assert_eq!(sniff_image_mime(b""), None);

    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Mine", None, None, now).unwrap();
    let blobs = blobs();
    assert!(matches!(
        store.set_cover_image(&list.id, b"GIF89a....", &blobs, now),
        Err(StoreError::Cover(CoverError::NotAnImage))
    ));
    let mut huge = JPEG.to_vec();
    huge.resize(MAX_IMAGE_BYTES + 1, 0);
    assert!(matches!(
        store.set_cover_image(&list.id, &huge, &blobs, now),
        Err(StoreError::Cover(CoverError::TooLarge { .. }))
    ));
    let file = blobs.root().join("huge.jpg");
    std::fs::write(&file, &huge).unwrap();
    assert!(matches!(
        store.set_cover_from_file(&list.id, &file, &blobs, now),
        Err(StoreError::Cover(CoverError::TooLarge { .. }))
    ));
    assert_eq!(store.playlist(&list.id).unwrap().unwrap().image_hash, None);
    assert!(store.images_needed_before_push().unwrap().is_empty());
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

#[test]
fn clearing_a_picture_falls_back_and_choosing_an_item_clears_it_too() {
    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Mine", None, None, now).unwrap();
    let blobs = blobs();
    store.set_cover_image(&list.id, JPEG, &blobs, now).unwrap();
    let cleared = store.clear_cover_image(&list.id, now).unwrap().unwrap();
    assert_eq!(cleared.image_hash, None);
    assert_eq!(cleared.image_mime, None);
    assert!(blobs.contains(&Store::image_hash(JPEG)), "the blob stays");

    store.set_cover_image(&list.id, JPEG, &blobs, now).unwrap();
    let chosen = store
        .set_artwork(Some("album-2"), &list.id, now)
        .unwrap()
        .unwrap();
    assert_eq!(chosen.image_hash, None);
    assert_eq!(chosen.artwork_item_id.as_deref(), Some("album-2"));
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

#[test]
fn the_same_picture_twice_is_one_file() {
    let blobs = blobs();
    let first = blobs.write(PNG).unwrap();
    let second = blobs.write(PNG).unwrap();
    assert_eq!(first, second);
    assert_eq!(std::fs::read_dir(blobs.root()).unwrap().count(), 1);
    assert!(blobs.remove(&first).unwrap());
    assert!(!blobs.remove(&first).unwrap());
    std::fs::remove_dir_all(blobs.root()).unwrap();
}
