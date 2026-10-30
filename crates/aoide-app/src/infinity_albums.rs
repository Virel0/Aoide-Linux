//! Infinity, album by album: when the queue runs short, whole records are
//! added instead of single songs, chosen by this listener's taste.
//!
//! The Electron app's albums mode, ported rule for rule
//! (`infinity-taste.ts`, `chooseAlbumsByTaste`): a pool several times what is
//! wanted (the albums of the songs Melete's `/aoide/next` would play after
//! the one playing, then the artist's, then any), each album judged on its
//! tracks — its genres
//! are its tracks' genres, its finish counts their sum, and it counts as just
//! heard when any track on it was — and the best few queued whole.
//!
//! **Why not `/aoide/next`.** Melete ranks songs; an album answer is not
//! something it gives. Taking its songs' albums would let one strong song pull
//! in a weak record, and the desktop's albums mode doesn't ask it either. So
//! with this setting on Infinity ranks albums here, from the local history,
//! whatever `/aoide/next` says; with it off, songs from `/aoide/next` as
//! before.

use std::collections::{HashMap, HashSet};

use aoide_melete::{Item, MeleteClient};
use aoide_sidecar::{NextMode, NextRequest, SidecarClient};
use aoide_store::{FinishCounts, TasteCandidate, TasteProfile, TasteRanking};
use iced::Task;

use crate::app::{App, Message};
use crate::playback::infinity::TopUp;
use crate::store_worker::StoreHandle;

/// How far back the taste profile reads.
const WINDOW_DAYS: i64 = 90;
/// Albums added each time the queue runs short.
pub const ALBUMS_PER_TOP_UP: usize = 2;
/// How many times that to gather, so the ranking has something to reject.
pub const POOL_MULTIPLIER: usize = 5;

/// What the album playing is, for gathering the pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub track: String,
    pub album: Option<String>,
    pub artist: Option<String>,
}

/// The albums to queue, best first, judged by the tracks on them. With no
/// history, or no tracklists at all, the pool's own order stands.
pub fn choose_albums_by_taste(
    album_ids: &[String],
    tracks: &[Item],
    profile: &TasteProfile,
    track_finish: &HashMap<String, FinishCounts>,
    limit: usize,
) -> Vec<String> {
    if tracks.is_empty() || profile.is_empty() {
        return album_ids.iter().take(limit).cloned().collect();
    }
    let mut by_album: HashMap<&str, Vec<&Item>> = HashMap::new();
    for track in tracks {
        if let Some(album) = track.album_id.as_deref() {
            by_album.entry(album).or_default().push(track);
        }
    }
    let mut candidates = Vec::with_capacity(album_ids.len());
    let mut finish = HashMap::new();
    let mut recent = HashSet::new();
    for album in album_ids {
        let on_it = by_album
            .get(album.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let artist = on_it
            .iter()
            .map(|t| taste_artist(t))
            .find(|name| !name.is_empty())
            .unwrap_or_default();
        let mut genres: Vec<String> = Vec::new();
        for genre in on_it.iter().flat_map(|t| t.genres.iter()) {
            if !genres.contains(genre) {
                genres.push(genre.clone());
            }
        }
        candidates.push(TasteCandidate::new(album.clone(), artist, &genres));
        let total = on_it.iter().fold(FinishCounts::default(), |sum, t| {
            sum + track_finish.get(&t.id).copied().unwrap_or_default()
        });
        finish.insert(album.clone(), total);
        if on_it.iter().any(|t| profile.recent.contains(&t.id)) {
            recent.insert(album.clone());
        }
    }
    let profile = TasteProfile {
        recent,
        ..profile.clone()
    };
    TasteRanking::rank(&candidates, &profile, &finish, &HashMap::new(), limit)
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// The album artist where there is one: the name the profile's weights are
/// keyed on.
fn taste_artist(item: &Item) -> String {
    item.album_artist
        .clone()
        .filter(|a| !a.is_empty())
        .or_else(|| item.artists.first().cloned())
        .unwrap_or_default()
}

/// Adds `ids` to `pool` in order, each once, none already queued or playing,
/// until the pool holds `count`.
fn gather(
    pool: &mut Vec<String>,
    ids: impl IntoIterator<Item = String>,
    skip: &HashSet<String>,
    count: usize,
) {
    for id in ids {
        if pool.len() >= count {
            return;
        }
        if !skip.contains(&id) && !pool.contains(&id) {
            pool.push(id);
        }
    }
}

/// The pool: the albums of what `/aoide/next` would play after the song
/// playing, the artist's, then any, without the ones already queued.
pub async fn pool(
    client: &MeleteClient,
    sidecar: Option<&SidecarClient>,
    seed: &Seed,
    queued_albums: &HashSet<String>,
    count: usize,
) -> Vec<String> {
    let mut pool = Vec::new();
    let mut skip = queued_albums.clone();
    if let Some(album) = &seed.album {
        skip.insert(album.clone());
    }
    let request = NextRequest::new(
        &seed.track,
        &[],
        &[],
        NextMode::Infinity,
        (count * 4) as u32,
    );
    if let Some(sidecar) = sidecar
        && let Ok(next) = sidecar.next(&request).await
    {
        let ids: Vec<String> = next.candidates.into_iter().map(|c| c.id).collect();
        if let Ok(songs) = client.items_by_ids(&ids).await {
            gather(
                &mut pool,
                songs.into_iter().filter_map(|t| t.album_id),
                &skip,
                count,
            );
        }
    }
    if pool.len() < count
        && let Some(artist) = &seed.artist
        && let Ok(albums) = client.artist_albums(artist).await
    {
        gather(&mut pool, albums.into_iter().map(|a| a.id), &skip, count);
    }
    if pool.len() < count
        && let Ok(albums) = crate::song_filter::sample_albums(client, count).await
    {
        gather(&mut pool, albums.into_iter().map(|a| a.id), &skip, count);
    }
    pool
}

/// One top-up: the pool, its tracklists, the ranking, and the chosen albums'
/// songs in album order with the reason on each.
pub async fn top_up(
    client: MeleteClient,
    sidecar: Option<SidecarClient>,
    store: StoreHandle,
    seed: Seed,
    queued_albums: HashSet<String>,
    now: aoide_store::Millis,
) -> TopUp {
    let albums = pool(
        &client,
        sidecar.as_ref(),
        &seed,
        &queued_albums,
        ALBUMS_PER_TOP_UP * POOL_MULTIPLIER,
    )
    .await;
    let mut tracks: Vec<Item> = Vec::new();
    for album in &albums {
        if let Ok(on_it) = client.album_tracks(album).await {
            tracks.extend(on_it);
        }
    }
    let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
    let since = now - WINDOW_DAYS * 24 * 60 * 60 * 1000;
    let (profile, finish, hidden) = store
        .call(move |w| {
            (
                w.store.taste_profile(since, 200, 40).unwrap_or_default(),
                w.store.finish_counts(&ids).unwrap_or_default(),
                w.store.not_interested(&ids).unwrap_or_default(),
            )
        })
        .await
        .unwrap_or_default();
    let chosen = choose_albums_by_taste(&albums, &tracks, &profile, &finish, ALBUMS_PER_TOP_UP);
    queue_order(&chosen, tracks, &hidden)
}

/// The chosen albums' songs, album after album in disc and track order, the
/// Not Interested ones left out, each with why it is there.
pub fn queue_order(
    chosen: &[String],
    tracks: Vec<Item>,
    hidden: &std::collections::BTreeSet<String>,
) -> TopUp {
    let mut by_album: HashMap<String, Vec<Item>> = HashMap::new();
    for track in tracks {
        if let Some(album) = track.album_id.clone() {
            by_album.entry(album).or_default().push(track);
        }
    }
    let mut items = Vec::new();
    let mut reasons = HashMap::new();
    for album in chosen {
        let Some(mut on_it) = by_album.remove(album) else {
            continue;
        };
        on_it.sort_by_key(|t| {
            (
                t.disc_number.unwrap_or(1),
                t.track_number.unwrap_or(i32::MAX),
            )
        });
        for track in on_it.into_iter().filter(|t| !hidden.contains(&t.id)) {
            let name = track.album.clone().unwrap_or_default();
            reasons.insert(
                track.id.clone(),
                format!("A whole album you might like: {name}"),
            );
            items.push(track);
        }
    }
    TopUp {
        items,
        reasons,
        sidecar_absent: false,
    }
}

impl App {
    /// Infinity's top-up in albums mode, for the track playing now.
    pub(crate) fn top_up_with_albums(&mut self) -> Task<Message> {
        let current = self.playback.current().map(|(_, q)| q.meta.clone());
        let queued_albums: HashSet<String> =
            (0..self.playback.queue().map_or(0, |q| q.entries().len()))
                .filter_map(|i| {
                    self.playback
                        .queued_at(i)
                        .and_then(|q| q.meta.album_id.clone())
                })
                .collect();
        let store = self.store.clone();
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let Some(current) = current else {
            return Task::none();
        };
        let seed = Seed {
            track: current.id,
            album: current.album_id,
            artist: current.artist_id,
        };
        let client = signed.connection.client.clone();
        let sidecar = signed.sidecar.clone();
        Task::perform(
            top_up(
                client,
                sidecar,
                store,
                seed,
                queued_albums,
                aoide_store::now_millis(),
            ),
            |found| Message::ToppedUp(Box::new(found)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn track(id: &str, album: &str, artist: &str, genre: &str, n: i32) -> Item {
        Item {
            id: id.into(),
            name: id.into(),
            album_id: Some(album.into()),
            album: Some(format!("Album {album}")),
            album_artist: Some(artist.into()),
            genres: vec![genre.into()],
            track_number: Some(n),
            ..Item::default()
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn with_no_history_the_pools_order_stands() {
        let tracks = [track("a1", "A", "x", "rock", 1)];
        let chosen = choose_albums_by_taste(
            &ids(&["B", "A", "C"]),
            &tracks,
            &TasteProfile::default(),
            &HashMap::new(),
            2,
        );
        assert_eq!(chosen, ["B", "A"]);
        let profile = TasteProfile {
            genres: HashMap::from([("rock".to_owned(), 1.0)]),
            ..TasteProfile::default()
        };
        // No tracklists at all: the fetch failed, not a reason to sort by id.
        assert_eq!(
            choose_albums_by_taste(&ids(&["B", "A"]), &[], &profile, &HashMap::new(), 2),
            ["B", "A"]
        );
    }

    #[test]
    fn a_record_is_judged_on_its_tracks() {
        let tracks = [
            track("a1", "A", "Mara Vey", "folk", 1),
            track("b1", "B", "Odd Hours", "jazz", 1),
            track("b2", "B", "Odd Hours", "indie", 2),
            track("c1", "C", "Someone", "metal", 1),
        ];
        let profile = TasteProfile {
            genres: HashMap::from([("indie".to_owned(), 1.0), ("folk".to_owned(), 0.6)]),
            ..TasteProfile::default()
        };
        // B's best genre (indie) wins, over A's folk.
        let chosen = choose_albums_by_taste(
            &ids(&["A", "B", "C"]),
            &tracks,
            &profile,
            &HashMap::new(),
            2,
        );
        assert_eq!(chosen, ["B", "A"]);
    }

    #[test]
    fn an_album_heard_this_afternoon_waits_and_finishes_add_up() {
        let tracks = [
            track("a1", "A", "x", "indie", 1),
            track("b1", "B", "y", "indie", 1),
            track("b2", "B", "y", "indie", 2),
        ];
        let mut profile = TasteProfile {
            genres: HashMap::from([("indie".to_owned(), 1.0)]),
            ..TasteProfile::default()
        };
        profile.recent.insert("b2".to_owned());
        assert_eq!(
            choose_albums_by_taste(&ids(&["B", "A"]), &tracks, &profile, &HashMap::new(), 1),
            ["A"]
        );
        profile.recent.clear();
        // Summed over the record: A finishes 1 of 3, B 3 of 3.
        let finish = HashMap::from([
            ("a1".to_owned(), FinishCounts::new(3, 1)),
            ("b1".to_owned(), FinishCounts::new(2, 2)),
            ("b2".to_owned(), FinishCounts::new(1, 1)),
        ]);
        assert_eq!(
            choose_albums_by_taste(&ids(&["A", "B"]), &tracks, &profile, &finish, 1),
            ["B"]
        );
    }

    #[test]
    fn chosen_albums_queue_whole_in_track_order_without_hidden_songs() {
        let tracks = vec![
            track("b2", "B", "y", "indie", 2),
            track("a1", "A", "x", "indie", 1),
            track("b1", "B", "y", "indie", 1),
            track("b3", "B", "y", "indie", 3),
        ];
        let hidden = BTreeSet::from(["b3".to_owned()]);
        let found = queue_order(&ids(&["B", "A"]), tracks, &hidden);
        let order: Vec<&str> = found.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, ["b1", "b2", "a1"]);
        assert_eq!(found.reasons["a1"], "A whole album you might like: Album A");
    }

    #[test]
    fn the_pool_skips_what_is_queued_and_holds_each_album_once() {
        let mut pool = Vec::new();
        let skip = HashSet::from(["Q".to_owned()]);
        gather(&mut pool, ids(&["A", "Q", "A", "B"]), &skip, 10);
        gather(&mut pool, ids(&["C", "B", "D"]), &skip, 3);
        assert_eq!(pool, ["A", "B", "C"]);
    }
}
