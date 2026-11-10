//! Which answers show the shelf, what its tiles hold and play, and the steps
//! of asking for a song. Nothing is sent: the tasks are dropped.

use aoide_melete::{
    Because, BecauseKind, Device, Item, MeleteClient, RequestKind, RequestState, ServerAddress,
};

use super::*;
use crate::store_worker::StoreHandle;

fn because() -> Because {
    Because {
        kind: BecauseKind::Song,
        title: Some("Harbour Lights".into()),
        artist: "The Quiet Engines".into(),
    }
}

fn track(id: &str) -> ForYouItem {
    ForYouItem {
        suggestion: Suggestion::Playable(Box::new(Item {
            id: id.into(),
            name: format!("Song {id}"),
            album_id: Some(format!("al-{id}")),
            has_cover: true,
            ..Item::default()
        })),
        because: because(),
    }
}

fn song(id: &str, poster: bool) -> ForYouSong {
    ForYouSong {
        mb_recording_id: id.into(),
        title: "Paper Boats".into(),
        artist: "Northern Static".into(),
        mb_artist_ids: Vec::new(),
        release: None,
        poster: poster.then(|| format!("/api/v1/music/release-groups/{id}/cover")),
    }
}

fn to_ask(id: &str, poster: bool) -> ForYouItem {
    ForYouItem {
        suggestion: Suggestion::Requestable(song(id, poster)),
        because: Because {
            kind: BecauseKind::Artist,
            title: None,
            artist: "The Quiet Engines".into(),
        },
    }
}

fn asked_for(id: &str) -> ForYouItem {
    ForYouItem {
        suggestion: Suggestion::Requested {
            song: song(id, false),
            request_id: Some(3),
        },
        because: because(),
    }
}

fn list(state: ForYouState, items: Vec<ForYouItem>) -> Result<Option<ForYou>, Failure> {
    Ok(Some(ForYou {
        state,
        made_at: Some(1_790_000_000),
        items,
    }))
}

#[test]
fn only_a_ready_list_with_songs_is_shown() {
    assert!(shelf(Ok(None)).is_none(), "an older server");
    for state in [
        ForYouState::Off,
        ForYouState::NoHistory,
        ForYouState::Waiting,
    ] {
        assert!(shelf(list(state, vec![track("1")])).is_none(), "{state:?}");
    }
    assert!(shelf(list(ForYouState::Ready, Vec::new())).is_none());
    assert!(shelf(Err(Failure::new("The server didn't answer.", None))).is_none());
    assert!(shelf(list(ForYouState::Ready, vec![track("1")])).is_some());
}

#[test]
fn tiles_hold_their_kind_picture_and_seed() {
    let items = vec![
        track("1"),
        to_ask("rec-1", true),
        asked_for("rec-2"),
        track("2"),
        to_ask("rec-3", false),
    ];
    let shelf = shelf(list(ForYouState::Ready, items)).unwrap();
    let kinds: Vec<&Kind> = shelf.entries.iter().map(|e| &e.kind).collect();
    assert_eq!(kinds[0], &Kind::Play(0));
    assert_eq!(kinds[1], &Kind::Ask(song("rec-1", true)));
    assert_eq!(kinds[2], &Kind::Asked(song("rec-2", false)));
    assert_eq!(kinds[3], &Kind::Play(1));
    assert_eq!(
        shelf.entries[0].art,
        Some(ArtKey::sized(
            "al-1",
            aoide_melete::COVER_TAG,
            crate::artwork::TILE
        ))
    );
    assert_eq!(
        shelf.entries[1].art,
        Some(crate::posters::key_at(
            "/api/v1/music/release-groups/rec-1/cover",
            crate::artwork::TILE
        ))
    );
    assert_eq!(shelf.entries[4].art, None, "the placeholder");
    assert_eq!(shelf.entries[0].because, "Harbour Lights");
    assert_eq!(shelf.entries[1].because, "The Quiet Engines");
    assert_eq!(shelf.artwork().count(), 3);
}

#[test]
fn the_playable_songs_are_what_a_tile_plays_from() {
    let items = vec![to_ask("rec-1", false), track("1"), track("2")];
    let shelf = shelf(list(ForYouState::Ready, items)).unwrap();
    let ids: Vec<&str> = shelf.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["1", "2"]);
}

#[test]
fn the_shelfs_menus_float_over_the_page() {
    assert!(floats(&format!("{MENU}4:0")));
    assert!(!floats("tile:al1"));
    assert!(!floats("4:0"));
}

fn client() -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse("http://127.0.0.1:9").unwrap(),
        Device::new("Test", "device-1"),
    )
    .unwrap()
}

fn run(shelf: &mut Shelf, message: Message) {
    let client = client();
    let store = StoreHandle::in_memory("device-1").unwrap();
    let _ = shelf.update(
        message,
        &Ctx {
            client: &client,
            store: &store,
            key: 1,
        },
    );
}

fn request() -> MusicRequest {
    MusicRequest {
        id: 31,
        kind: RequestKind::Song,
        title: "Paper Boats".into(),
        year: None,
        artist: Some("Northern Static".into()),
        state: RequestState::Requested,
        reason: None,
        created_at: 1_790_000_000,
        decided_at: None,
        decided_by: None,
        progress: None,
    }
}

#[test]
fn asking_goes_one_at_a_time_and_keeps_the_answer() {
    let items = vec![track("1"), asked_for("rec-2"), to_ask("rec-1", false)];
    let mut shelf = shelf(list(ForYouState::Ready, items)).unwrap();
    run(&mut shelf, Message::Ask(0));
    run(&mut shelf, Message::Ask(1));
    assert_eq!(shelf.asking, None, "only a song to ask for is asked for");
    run(&mut shelf, Message::Ask(2));
    assert_eq!(shelf.asking, Some(2));
    run(&mut shelf, Message::Asked(2, Ok(Box::new(request()))));
    assert_eq!(shelf.asking, None);
    assert_eq!(shelf.asked.len(), 1);
    assert!(matches!(&shelf.asked[0], (2, Ok(r)) if r.id == 31));
}

#[test]
fn a_failed_ask_says_why() {
    let mut shelf = shelf(list(ForYouState::Ready, vec![to_ask("rec-1", false)])).unwrap();
    run(&mut shelf, Message::Ask(0));
    let why = Failure::new("It's been asked for already.", None);
    run(&mut shelf, Message::Asked(0, Err(why)));
    assert_eq!(shelf.asking, None);
    assert!(matches!(&shelf.asked[0], (0, Err(w)) if w == "It's been asked for already."));
}
