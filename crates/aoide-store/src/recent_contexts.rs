//! Something a listener was *in* — an album, a playlist, a mix, a station, or a song
//! picked out of an album — so Home can open on the last few and resume one in a tap. The iOS `RecentContexts` and the
//! desktop `recent-contexts.ts`, which agree on every rule here.
//!
//! A context is a thing playback **started from**: record one when playback starts
//! from that thing's own page (an album's Play, a playlist's Shuffle, a station), never
//! from a shuffle-all or a search result, because a grid of six that fills up with
//! accidents is a grid nobody looks at.
//!
//! Local to the device on purpose — what the desk was in is not what the phone was in,
//! and the queue handover covers "carry on from the other one" — and kept **per
//! server**, because an album id means nothing on another server. [`RecentContexts`]
//! is a plain value the app saves as JSON with [`RecentContexts::save`].

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::error::Result;
use crate::handoff::ResumableQueue;

/// What kind of thing a recent context is, which says where its id is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecentContextKind {
    /// A Melete album.
    Album,
    /// A Jellyfin playlist, from before Melete.
    JellyfinPlaylist,
    /// An Aoide playlist. Spelled as the phone spells it; the desktop's
    /// `aoidePlaylist` is read as the same thing.
    #[serde(alias = "aoidePlaylist")]
    LocalPlaylist,
    /// A described mix; its `id` and `name` are the description that was typed.
    Mix,
    /// A station from an item, chosen by `/aoide/next`; see [`RecentContext::seed`].
    Station,
    /// One song chosen from an album's own list, resumed from that song on through
    /// [`RecentContext::album_id`].
    Song,
}

impl RecentContextKind {
    /// The camelCase spelling written to disk.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::JellyfinPlaylist => "jellyfinPlaylist",
            Self::LocalPlaylist => "localPlaylist",
            Self::Mix => "mix",
            Self::Station => "station",
            Self::Song => "song",
        }
    }

    /// The word under a tile that has no subtitle of its own.
    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::JellyfinPlaylist | Self::LocalPlaylist => "Playlist",
            Self::Mix => "Mix",
            Self::Station => "Station",
            Self::Song => "Song",
        }
    }
}

/// What seeded a station: an album or an artist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StationSeed {
    /// Seeded from an album.
    Album,
    /// Seeded from an artist.
    Artist,
}

/// Something played from recently: an album, a playlist, a mix, a station or a song.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentContext {
    /// What kind of thing it is.
    pub kind: RecentContextKind,
    /// The album, playlist or seed item id, in whichever store the kind names; for a
    /// mix, its description.
    pub id: String,
    /// The name shown, as it was when last opened.
    pub name: String,
    /// A second line to show instead of the kind's label, such as an album's artist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// The Melete item whose cover stands for this context. Leave unset for
    /// an album or Jellyfin playlist: its own id draws it. A song's is its album.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_item_id: Option<String>,
    /// Only for a station: which kind of thing seeded it, so it can be seeded again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<StationSeed>,
    /// Only for an Aoide playlist: whether it is rules rather than a list, which is
    /// what its plays are filed under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<bool>,
    /// Only for a song: the album it was picked from, which it is resumed in. Lists
    /// saved before songs were kept have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    /// This device's clock. Only ever compared on this device.
    pub last_opened: Millis,
}

impl RecentContext {
    /// A context with no subtitle, artwork, seed, smart flag or album.
    pub fn new(
        kind: RecentContextKind,
        id: impl Into<String>,
        name: impl Into<String>,
        last_opened: Millis,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            name: name.into(),
            subtitle: None,
            artwork_item_id: None,
            seed: None,
            smart: None,
            album_id: None,
            last_opened,
        }
    }

    /// A mix is its description: what was typed, what the tile says, and what the mix
    /// page is handed back to make it again.
    pub fn mix(description: &str, last_opened: Millis) -> Self {
        let description = description.trim();
        Self::new(
            RecentContextKind::Mix,
            description,
            description,
            last_opened,
        )
    }

    /// A station seeded from `item_id`, an album or an artist as `seed` says.
    pub fn station(
        seed: StationSeed,
        item_id: impl Into<String>,
        name: impl Into<String>,
        last_opened: Millis,
    ) -> Self {
        let mut context = Self::new(RecentContextKind::Station, item_id, name, last_opened);
        context.seed = Some(seed);
        context
    }

    /// A song picked from `album_id`'s own list, drawn with that album's cover.
    pub fn song(
        id: impl Into<String>,
        name: impl Into<String>,
        album_id: impl Into<String>,
        last_opened: Millis,
    ) -> Self {
        let album_id = album_id.into();
        let mut context = Self::new(RecentContextKind::Song, id, name, last_opened);
        context.artwork_item_id = Some(album_id.clone());
        context.album_id = Some(album_id);
        context
    }

    /// One entry per thing, whatever its name is today.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }

    /// The album this stands for: an album's own id, or a song's album. One album is
    /// one tile, whether it was last played whole or from one of its songs.
    pub fn album(&self) -> Option<&str> {
        match self.kind {
            RecentContextKind::Album => Some(self.id.as_str()),
            RecentContextKind::Song => self.album_id.as_deref(),
            _ => None,
        }
    }

    /// The line under the name: the subtitle, or what kind of thing it is.
    pub fn detail(&self) -> &str {
        self.subtitle.as_deref().unwrap_or(self.kind.label())
    }
}

/// The recent contexts of one server, newest first, one per thing, bounded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentContextList {
    contexts: Vec<RecentContext>,
}

impl RecentContextList {
    /// Enough that a busy week does not push Monday's album out before the weekend,
    /// and that the sixth tile has something to be replaced by.
    pub const CAPACITY: usize = 24;

    /// Every context, newest first.
    pub fn contexts(&self) -> &[RecentContext] {
        &self.contexts
    }

    /// Records a visit: the same thing again moves to the front with its new name. An
    /// album played whole replaces a song of it played before, and a song replaces its
    /// album and any other song of it.
    pub fn remember(&mut self, context: RecentContext) {
        let key = context.key();
        let album = context.album().map(str::to_owned);
        self.contexts.retain(|existing| {
            existing.key() != key && (album.is_none() || existing.album() != album.as_deref())
        });
        self.contexts.insert(0, context);
        self.contexts.truncate(Self::CAPACITY);
    }

    /// Removes the context with this [`RecentContext::key`], if any.
    pub fn forget(&mut self, key: &str) {
        self.contexts.retain(|existing| existing.key() != key);
    }

    /// The most recent `limit`, newest first.
    pub fn latest(&self, limit: usize) -> &[RecentContext] {
        &self.contexts[..limit.min(self.contexts.len())]
    }
}

/// Every server's recent contexts, as one value to keep on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentContexts {
    by_server: BTreeMap<String, RecentContextList>,
}

impl RecentContexts {
    /// Records that playback started from `context` on `server_id`. A context with no
    /// id or no name is ignored: there is nothing a tile could resume or say.
    pub fn remember(&mut self, server_id: &str, context: RecentContext) {
        if server_id.is_empty() || context.id.trim().is_empty() || context.name.trim().is_empty() {
            return;
        }
        self.by_server
            .entry(server_id.to_owned())
            .or_default()
            .remember(context);
    }

    /// "Remove from Recents", or a local playlist that turned out to be deleted.
    pub fn forget(&mut self, server_id: &str, key: &str) {
        if let Some(list) = self.by_server.get_mut(server_id) {
            list.forget(key);
        }
    }

    /// Everything kept for a server, newest first.
    pub fn list(&self, server_id: &str) -> &[RecentContext] {
        self.by_server
            .get(server_id)
            .map_or(&[], RecentContextList::contexts)
    }

    /// The newest `limit` for a server.
    pub fn latest(&self, server_id: &str, limit: usize) -> &[RecentContext] {
        self.by_server
            .get(server_id)
            .map_or(&[], |list| list.latest(limit))
    }

    /// Forgets a server altogether — on sign-out from it, or when the account changes.
    pub fn forget_server(&mut self, server_id: &str) {
        self.by_server.remove(server_id);
    }

    /// Reads what [`save`](Self::save) wrote. A missing or unreadable file is an
    /// empty list: losing the grid costs a few taps, refusing to start costs the app.
    pub fn load(path: impl AsRef<Path>) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes atomically: to a sibling file, then renamed over the old one, so a crash
    /// mid-write leaves the previous list rather than half of one.
    ///
    /// # Errors
    ///
    /// Fails when the list does not encode or the file cannot be written or renamed; the
    /// previous file is left as it was then.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = serde_json::to_vec(self)?;
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

// MARK: - The resume grid

/// How many tiles the Home grid shows: two rows of three.
pub const RESUME_GRID_LIMIT: usize = 6;

/// One tile of the resume grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeTile {
    /// Another device's queue — "Pick up from {device}". Resume it with
    /// [`ResumableQueue::pick_up`].
    Handoff(ResumableQueue),
    /// Something opened recently on this device.
    Context(RecentContext),
}

impl ResumeTile {
    /// The tile's name: "Pick up from {device}" for a handoff, the context's name otherwise.
    pub fn title(&self) -> String {
        match self {
            Self::Handoff(queue) => queue.title(),
            Self::Context(context) => context.name.clone(),
        }
    }

    /// The line under the title.
    pub fn detail(&self) -> String {
        match self {
            Self::Handoff(queue) => match queue.track_ids.len() {
                1 => "1 track".to_owned(),
                count => format!("{count} tracks"),
            },
            Self::Context(context) => context.detail().to_owned(),
        }
    }

    /// The Melete item whose cover the tile draws, when there is one. For a handoff
    /// it is the other device's current track: covers belong to albums, so the app
    /// looks that track up and draws its album's.
    pub fn artwork_item_id(&self) -> Option<&str> {
        match self {
            Self::Handoff(queue) => queue.current_track_id(),
            Self::Context(context) => match context.kind {
                RecentContextKind::Album | RecentContextKind::JellyfinPlaylist => Some(
                    context
                        .artwork_item_id
                        .as_deref()
                        .unwrap_or(context.id.as_str()),
                ),
                RecentContextKind::Song => context
                    .artwork_item_id
                    .as_deref()
                    .or(context.album_id.as_deref()),
                _ => context.artwork_item_id.as_deref(),
            },
        }
    }
}

/// The tiles the grid draws: up to `limit`, newest first, with another device's
/// queue first when it was playing **more recently** than anything this device was
/// in. A tie goes to this device — the tile it already had is one tap, and a queue
/// this machine just wrote would be a handover to itself.
///
/// "More recently" compares [`ResumableQueue::played_at`] — the server's age read
/// against this device's clock — with this device's own timestamps, so the other
/// device's clock is never trusted. Pass the first of `resumable_queues`.
pub fn resume_tiles(
    contexts: &[RecentContext],
    handoff: Option<&ResumableQueue>,
    limit: usize,
) -> Vec<ResumeTile> {
    let newest_here = contexts.iter().map(|c| c.last_opened).max();
    let lead = handoff.filter(|queue| newest_here.is_none_or(|newest| queue.played_at() > newest));
    let mut tiles: Vec<ResumeTile> = lead
        .map(|queue| ResumeTile::Handoff(queue.clone()))
        .into_iter()
        .collect();
    let mut newest_first: Vec<&RecentContext> = contexts.iter().collect();
    newest_first.sort_by_key(|context| std::cmp::Reverse(context.last_opened));
    tiles.extend(
        newest_first
            .into_iter()
            .map(|context| ResumeTile::Context(context.clone())),
    );
    tiles.truncate(limit);
    tiles
}
