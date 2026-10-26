//! The resume grid: its tiles and their covers, and each tile's tracks,
//! found where its kind keeps them; or another device's queue, picked up.

use std::collections::{HashMap, HashSet};

use aoide_jellyfin::{Item, JellyfinClient, MixSeed};
use aoide_sidecar::resumable_queues;
use aoide_store::{
    Calendar, PlaySource, RESUME_GRID_LIMIT, RecentContext, RecentContextKind, ResumableQueue,
    ResumeTile, StationSeed, resume_tiles,
};

use super::PickedUp;
use crate::artwork::ArtKey;
use crate::playback::tracks::TrackMeta;
use crate::store_worker::StoreHandle;

/// How many tracks a station resumed from the grid starts with.
pub const STATION_LENGTH: u32 = 100;

/// A tile of the grid, with the cover to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub tile: ResumeTile,
    pub art: Option<ArtKey>,
}

/// A tile's tracks, found.
#[derive(Debug, Clone)]
pub struct Resumed {
    pub tracks: Vec<TrackMeta>,
    pub start: usize,
    pub source: PlaySource,
    /// Where in the start track to begin (a pick-up).
    pub seek_ms: Option<u64>,
    /// The queue this picks up.
    pub picked_up: Option<PickedUp>,
    /// The context to move to the front again.
    pub context: Option<RecentContext>,
}

/// The station to play for a station context: the seed's own Instant Mix
/// when it says what it was, else the kind-blind one.
pub fn station_seed(context: &RecentContext) -> Option<MixSeed> {
    match context.seed? {
        StationSeed::Album => Some(MixSeed::Album(context.id.clone())),
        StationSeed::Artist => Some(MixSeed::Artist(context.id.clone())),
    }
}

/// The other device's queue in its order, as far as this server still has
/// it, starting at its track. `None` when nothing in it can play here.
pub fn plan_pick_up(queue: &ResumableQueue, found: &[Item]) -> Option<Resumed> {
    let by_id: HashMap<&str, &Item> = found.iter().map(|i| (i.id.as_str(), i)).collect();
    let pick = queue.pick_up(|id| by_id.contains_key(id))?;
    let tracks: Vec<TrackMeta> = pick
        .track_ids
        .iter()
        .filter_map(|id| {
            by_id
                .get(id.as_str())
                .map(|item| TrackMeta::from_item(item))
        })
        .collect();
    let seek = u64::try_from(pick.elapsed_ms).unwrap_or(0);
    Some(Resumed {
        tracks,
        start: pick.start_index,
        source: PlaySource::Unknown,
        seek_ms: (seek > 0).then_some(seek),
        picked_up: Some(PickedUp::of(queue)),
        context: None,
    })
}

/// Each tile's cover, from the items its artwork ids name.
pub fn cards(tiles: Vec<ResumeTile>, items: &[Item]) -> Vec<Card> {
    let art: HashMap<&str, ArtKey> = items
        .iter()
        .filter_map(|item| {
            let (id, tag) = item.artwork_source()?;
            Some((item.id.as_str(), ArtKey::new(id, tag)))
        })
        .collect();
    tiles
        .into_iter()
        .map(|tile| Card {
            art: tile.artwork_item_id().and_then(|id| art.get(id).cloned()),
            tile,
        })
        .collect()
}

/// The grid's tiles: the newest queue another device put down, then this
/// device's contexts, with each tile's cover.
pub async fn load_cards(
    client: JellyfinClient,
    sidecar: Option<aoide_sidecar::SidecarClient>,
    store: std::sync::Arc<aoide_store::Store>,
    contexts: Vec<RecentContext>,
    picked_up: Option<PickedUp>,
    now: aoide_store::Millis,
) -> Vec<Card> {
    let queues = resumable_queues(sidecar.as_ref(), &store, now)
        .await
        .unwrap_or_default();
    let newest = queues
        .first()
        .filter(|queue| picked_up.as_ref() != Some(&PickedUp::of(queue)));
    let tiles = resume_tiles(&contexts, newest, RESUME_GRID_LIMIT);
    let ids: Vec<String> = tiles
        .iter()
        .filter_map(|t| t.artwork_item_id().map(str::to_owned))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let items = if ids.is_empty() {
        Vec::new()
    } else {
        client.items_by_ids(&ids).await.unwrap_or_default()
    };
    cards(tiles, &items)
}

/// A tile's tracks, from wherever its kind keeps them.
pub async fn resume(
    client: JellyfinClient,
    store: StoreHandle,
    tile: ResumeTile,
) -> Result<Resumed, String> {
    let context = match tile {
        ResumeTile::Handoff(queue) => {
            let found = client
                .items_by_ids(&queue.track_ids)
                .await
                .map_err(describe)?;
            return plan_pick_up(&queue, &found).ok_or_else(|| {
                format!(
                    "None of what {} was playing is on this server.",
                    queue.device_name
                )
            });
        }
        ResumeTile::Context(context) => context,
    };
    let (items, source) = match context.kind {
        RecentContextKind::Album => return album(&client, context).await,
        RecentContextKind::JellyfinPlaylist => (
            client.playlist_items(&context.id).await.map_err(describe)?,
            PlaySource::Playlist,
        ),
        RecentContextKind::LocalPlaylist => local_playlist(&client, &store, &context).await?,
        RecentContextKind::Station => station(&client, &store, &context).await?,
        RecentContextKind::Mix => {
            return Err(
                "A mix is made again from the Mixes screen, which this build doesn't have yet."
                    .to_owned(),
            );
        }
    };
    Ok(Resumed {
        tracks: items.iter().map(TrackMeta::from_item).collect(),
        start: 0,
        source,
        seek_ms: None,
        picked_up: None,
        context: Some(context),
    })
}

fn describe(e: aoide_jellyfin::Error) -> String {
    crate::failure::describe(&e).to_string()
}

/// An album's tracks, in the album's order.
async fn album(client: &JellyfinClient, context: RecentContext) -> Result<Resumed, String> {
    let mut tracks: Vec<TrackMeta> = client
        .album_tracks(&context.id)
        .await
        .map_err(describe)?
        .iter()
        .map(TrackMeta::from_item)
        .collect();
    crate::screens::album::album_order(&mut tracks);
    Ok(Resumed {
        tracks,
        start: 0,
        source: PlaySource::Album,
        seek_ms: None,
        picked_up: None,
        context: Some(context),
    })
}

/// An Aoide playlist's songs in its order (a smart one's evaluated now).
async fn local_playlist(
    client: &JellyfinClient,
    store: &StoreHandle,
    context: &RecentContext,
) -> Result<(Vec<Item>, PlaySource), String> {
    let id = context.id.clone();
    let found = store
        .call(move |w| local_playlist_ids(&w.store, &id))
        .await
        .ok_or_else(|| "the library is closing".to_owned())?;
    let Some((ids, smart)) = found? else {
        return Err(format!(
            "\u{201c}{}\u{201d} has been deleted.",
            context.name
        ));
    };
    let items = ordered(client.items_by_ids(&ids).await.map_err(describe)?, &ids);
    let source = if smart {
        PlaySource::Smart
    } else {
        PlaySource::Playlist
    };
    Ok((items, source))
}

/// A station's Instant Mix, made again, without the songs marked Not
/// Interested.
async fn station(
    client: &JellyfinClient,
    store: &StoreHandle,
    context: &RecentContext,
) -> Result<(Vec<Item>, PlaySource), String> {
    let items = match station_seed(context) {
        Some(seed) => client.instant_mix_from(&seed, STATION_LENGTH).await,
        None => client.instant_mix(&context.id, STATION_LENGTH).await,
    }
    .map_err(describe)?;
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let hidden = store
        .call(move |w| w.store.not_interested(&ids).unwrap_or_default())
        .await
        .unwrap_or_default();
    Ok((
        items
            .into_iter()
            .filter(|i| !hidden.contains(&i.id))
            .collect(),
        PlaySource::Unknown,
    ))
}

/// An Aoide playlist's track ids in play order, smart ones evaluated now, and
/// whether it is smart. `None` when it has been deleted.
pub fn local_playlist_ids(
    store: &aoide_store::Store,
    id: &str,
) -> Result<Option<(Vec<String>, bool)>, String> {
    let Some(playlist) = store.playlist(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if playlist.deleted {
        return Ok(None);
    }
    if playlist.is_smart {
        let ids = store
            .resolve_smart(&playlist, aoide_store::now_millis(), Calendar::Local)
            .map_err(|e| e.to_string())?;
        return Ok(Some((ids, true)));
    }
    let ids = store
        .items(id)
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|item| !item.is_unresolved())
        .map(|item| item.linked_jellyfin_id().to_owned())
        .collect();
    Ok(Some((ids, false)))
}

/// `items` in the order of `ids`, each as often as it is named.
pub fn ordered(items: Vec<Item>, ids: &[String]) -> Vec<Item> {
    let by_id: HashMap<String, Item> = items.into_iter().map(|i| (i.id.clone(), i)).collect();
    ids.iter().filter_map(|id| by_id.get(id).cloned()).collect()
}
