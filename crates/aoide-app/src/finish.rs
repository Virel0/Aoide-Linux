//! What this listener's own history says about a set of songs: how much of
//! it they finish, and which of an artist's songs they play most.
//!
//! The finish rate is the store's (`FinishRate`, shared with the phone and
//! the desktop): summed over the set, and absent below three decided listens,
//! because "0%" from two plays says something untrue.

use std::collections::HashMap;

use aoide_jellyfin::Item;
use aoide_store::{FinishCounts, FinishRate};

use crate::store_worker::StoreHandle;

/// How many songs an artist page lists as their top songs.
pub const TOP_SONGS: usize = 10;

/// "You finish 78% of this album's songs", or `None` without enough history.
pub fn finish_line(counts: impl IntoIterator<Item = FinishCounts>, of: &str) -> Option<String> {
    let total = counts
        .into_iter()
        .fold(FinishCounts::default(), |sum, c| sum + c);
    FinishRate::percent_description(total).map(|percent| format!("You finish {percent} of {of}"))
}

/// The finish line for these tracks, read on the store thread.
pub async fn finish_of(store: StoreHandle, ids: Vec<String>, of: &'static str) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    store
        .call(move |w| w.store.finish_counts(&ids).ok())
        .await
        .flatten()
        .and_then(|counts| finish_line(counts.into_values(), of))
}

/// An artist's most played songs: by this listener's plays in the history,
/// then by the server's count, each song once; songs nobody has played are
/// left out, so an unplayed artist shows no list rather than an arbitrary one.
pub fn top_songs(items: &[Item], local_plays: &HashMap<String, i64>, limit: usize) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    let mut ranked: Vec<(i64, u32, &Item)> = items
        .iter()
        .filter(|item| seen.insert(item.id.as_str()))
        .map(|item| {
            let local = local_plays.get(&item.id).copied().unwrap_or(0);
            let server = item.user_data.as_ref().map_or(0, |u| u.play_count);
            (local, server, item)
        })
        .filter(|(local, server, _)| *local > 0 || *server > 0)
        .collect();
    // Stable: ties keep the album order they came in.
    ranked.sort_by_key(|a| std::cmp::Reverse((a.0, a.1)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, item)| item.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_is_summed_and_absent_below_three_starts() {
        let counts = [FinishCounts::new(1, 1), FinishCounts::new(1, 0)];
        assert_eq!(
            finish_line(counts, "this album's songs"),
            None,
            "two starts"
        );
        let counts = [
            FinishCounts::new(2, 2),
            FinishCounts::new(1, 0),
            FinishCounts::new(1, 1),
        ];
        assert_eq!(
            finish_line(counts, "this album's songs").as_deref(),
            Some("You finish 75% of this album's songs")
        );
        assert_eq!(finish_line([], "x"), None);
    }

    fn item(id: &str, server_plays: u32) -> Item {
        serde_json::from_value(serde_json::json!({
            "Id": id, "Name": id, "Type": "Audio",
            "UserData": {"PlayCount": server_plays}
        }))
        .unwrap()
    }

    #[test]
    fn top_songs_rank_this_listeners_plays_first() {
        let items = [
            item("a", 9),
            item("b", 1),
            item("c", 0),
            item("d", 4),
            item("a", 9),
        ];
        let local = HashMap::from([("b".to_owned(), 5), ("c".to_owned(), 2)]);
        let top = top_songs(&items, &local, 3);
        let ids: Vec<&str> = top.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"]);
        assert!(top_songs(&[item("x", 0)], &HashMap::new(), 10).is_empty());
    }

    #[tokio::test]
    async fn the_store_says_how_much_is_finished() {
        let store = StoreHandle::in_memory("desk").unwrap();
        store
            .call(|w| {
                let track = aoide_store::TrackLink::new("t1".to_owned(), "k1".to_owned());
                for (n, completed) in [true, true, false].into_iter().enumerate() {
                    let at = 1_000_000 * (n as i64 + 1);
                    let heard = if completed { 180_000 } else { 5_000 };
                    w.store
                        .record_play(
                            &track,
                            at,
                            heard,
                            Some(180_000),
                            aoide_store::PlaySource::Album,
                            Some(completed),
                        )
                        .unwrap();
                }
            })
            .await
            .unwrap();
        let line = finish_of(store, vec!["t1".into(), "t2".into()], "this album's songs").await;
        assert_eq!(
            line.as_deref(),
            Some("You finish 67% of this album's songs")
        );
    }
}
