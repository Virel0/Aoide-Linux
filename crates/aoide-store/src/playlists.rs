//! Playlists, folders, the entries in them, and keeping entries linked across rescans.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::Millis;
use crate::error::Result;
use crate::fractional_index::FractionalIndex;
use crate::models::{Folder, Playlist, PlaylistItem, TrackRef};
use crate::new_id;
use crate::record::{Record, placeholders};
use crate::store::Store;

/// The pair an entry stores for every track: the id Jellyfin answers to today, and
/// the fingerprint that outlives it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackLink {
    pub jellyfin_id: String,
    pub content_key: String,
}

impl TrackLink {
    pub fn new(jellyfin_id: impl Into<String>, content_key: impl Into<String>) -> Self {
        Self {
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
        }
    }
}

impl From<&TrackRef> for TrackLink {
    fn from(track: &TrackRef) -> Self {
        Self::new(&track.jellyfin_id, &track.content_key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaylistError {
    #[error("no playlist {0}")]
    NoSuchPlaylist(String),
    #[error("no folder {0}")]
    NoSuchFolder(String),
    #[error("no playlist entry {0}")]
    NoSuchItem(String),
    #[error("entry {item} is not in playlist {playlist}")]
    ItemNotInPlaylist { item: String, playlist: String },
    /// A folder was asked to become its own ancestor.
    #[error("folder {folder} cannot move under {under}, which is inside it")]
    FolderCycle { folder: String, under: String },
    /// The neighbours bracket no gap: the wrong way round, or one is the row moving.
    #[error("those neighbours leave no gap to move into")]
    BadNeighbours,
}

/// What a relink pass did: `relinked` is the silent repair, `unresolved` is the one
/// that has to surface as "3 songs in this playlist are missing".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelinkReport {
    /// Entries examined — every live playlist row.
    pub checked: usize,
    /// Rewritten to a new Jellyfin id found by content key.
    pub relinked: usize,
    /// Previously marked unresolved and resolving again.
    pub recovered: usize,
    /// Newly marked unresolved: no id matched their fingerprint.
    pub unresolved: usize,
}

impl RelinkReport {
    pub fn changed(&self) -> usize {
        self.relinked + self.recovered + self.unresolved
    }
}

/// Stamped onto `jellyfin_id` when a relink pass finds no track for an entry.
///
/// Inside a column that already syncs, so it travels for free, and a build that has
/// never heard of it simply fails to resolve the id — which is the situation the entry
/// is in. Jellyfin ids are hex, so a colon cannot occur in a real one.
pub const UNRESOLVED_PREFIX: &str = "unresolved:";

impl PlaylistItem {
    pub fn is_unresolved(&self) -> bool {
        self.jellyfin_id.starts_with(UNRESOLVED_PREFIX)
    }

    /// The id to play or look up, marker stripped. The last id that resolved is kept,
    /// because the usual cause of a miss is an unmounted drive.
    pub fn linked_jellyfin_id(&self) -> &str {
        self.jellyfin_id
            .strip_prefix(UNRESOLVED_PREFIX)
            .unwrap_or(&self.jellyfin_id)
    }
}

// MARK: - Playlists

impl Store {
    /// Live playlists in display order, across every folder. Sort keys are generated
    /// against all playlists, so a folder's contents are a subsequence of this.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        self.read(|conn| Playlist::fetch_where(conn, "deleted = 0 ORDER BY sortIndex, rowid", []))
    }

    /// Live playlists filed directly under `folder_id`; `None` is the root.
    pub fn playlists_in_folder(&self, folder_id: Option<&str>) -> Result<Vec<Playlist>> {
        self.read(|conn| {
            Playlist::fetch_where(
                conn,
                "deleted = 0 AND folderId IS ? ORDER BY sortIndex, rowid",
                [folder_id],
            )
        })
    }

    /// A live playlist; a deleted one is `None`.
    pub fn playlist(&self, id: &str) -> Result<Option<Playlist>> {
        Ok(self
            .read(|conn| Playlist::fetch(conn, id))?
            .filter(|list| !list.deleted))
    }

    /// Whether a row with this id has ever existed here, deleted or not — so a deleted
    /// built-in is not created again under the same id and undeleted everywhere.
    pub fn playlist_row_exists(&self, id: &str) -> Result<bool> {
        Ok(self.read(|conn| Playlist::fetch(conn, id))?.is_some())
    }

    pub fn create_playlist(
        &self,
        name: &str,
        folder_id: Option<&str>,
        notes: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        self.create_playlist_with_id(&new_id(), name, folder_id, notes, now)
    }

    /// A caller-chosen id is for the built-ins, which every device creates under the
    /// same id so they merge rather than duplicate.
    pub fn create_playlist_with_id(
        &self,
        id: &str,
        name: &str,
        folder_id: Option<&str>,
        notes: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        if let Some(folder_id) = folder_id {
            self.require_folder(folder_id)?;
        }
        let mut list = Playlist::new(
            name,
            self.next_playlist_sort_index()?,
            now,
            self.device_id(),
        );
        list.id = id.to_owned();
        list.notes = notes.map(str::to_owned);
        list.folder_id = folder_id.map(str::to_owned);
        self.record(list, false, now)
    }

    /// Renames in place. The whole row goes into the op, and the receiving side merges
    /// per field, so a concurrent reorder elsewhere survives.
    pub fn rename_playlist(&self, id: &str, name: &str, now: Millis) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        list.name = name.to_owned();
        self.record(list, false, now)
    }

    pub fn set_notes(
        &self,
        notes: Option<&str>,
        playlist_id: &str,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(playlist_id)?;
        list.notes = notes.map(str::to_owned);
        self.record(list, false, now)
    }

    /// Files a playlist under a folder, or at the root, landing at the end of it.
    pub fn move_playlist(
        &self,
        id: &str,
        folder_id: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        if let Some(folder_id) = folder_id {
            self.require_folder(folder_id)?;
        }
        list.folder_id = folder_id.map(str::to_owned);
        list.sort_index = self.next_playlist_sort_index()?;
        self.record(list, false, now)
    }

    /// Soft-deletes a playlist **and its items**, one op each, items first — so a crash
    /// half way leaves a shortened playlist rather than orphans. Deleting twice mints
    /// nothing.
    pub fn delete_playlist(&self, id: &str, now: Millis) -> Result<()> {
        let list = self
            .read(|conn| Playlist::fetch(conn, id))?
            .ok_or_else(|| PlaylistError::NoSuchPlaylist(id.to_owned()))?;
        if list.deleted {
            return Ok(());
        }
        for item in self.items(id)? {
            self.record(item, true, now)?;
        }
        self.record(list, true, now)?;
        Ok(())
    }

    /// Repositions a playlist between two siblings; `None` on either side is the end.
    /// Passing neither leaves it where it is.
    pub fn reorder_playlist(
        &self,
        id: &str,
        after: Option<&str>,
        before: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        if after.is_none() && before.is_none() {
            return Ok(list);
        }
        if after == Some(id) || before == Some(id) {
            return Err(PlaylistError::BadNeighbours.into());
        }
        let lower = after
            .map(|n| self.require_playlist(n).map(|p| p.sort_index))
            .transpose()?;
        let upper = before
            .map(|n| self.require_playlist(n).map(|p| p.sort_index))
            .transpose()?;
        check_gap(lower.as_deref(), upper.as_deref())?;
        list.sort_index = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(list, false, now)
    }
}

// MARK: - Folders

impl Store {
    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.read(|conn| Folder::fetch_where(conn, "deleted = 0 ORDER BY sortIndex, rowid", []))
    }

    pub fn folders_in_parent(&self, parent_id: Option<&str>) -> Result<Vec<Folder>> {
        self.read(|conn| {
            Folder::fetch_where(
                conn,
                "deleted = 0 AND parentId IS ? ORDER BY sortIndex, rowid",
                [parent_id],
            )
        })
    }

    pub fn folder(&self, id: &str) -> Result<Option<Folder>> {
        Ok(self
            .read(|conn| Folder::fetch(conn, id))?
            .filter(|folder| !folder.deleted))
    }

    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
        now: Millis,
    ) -> Result<Folder> {
        if let Some(parent_id) = parent_id {
            self.require_folder(parent_id)?;
        }
        let mut folder = Folder::new(name, self.next_folder_sort_index()?, now, self.device_id());
        folder.parent_id = parent_id.map(str::to_owned);
        self.record(folder, false, now)
    }

    pub fn rename_folder(&self, id: &str, name: &str, now: Millis) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        folder.name = name.to_owned();
        self.record(folder, false, now)
    }

    /// Re-parents a folder, refusing any move that would make it its own ancestor.
    pub fn move_folder(&self, id: &str, parent_id: Option<&str>, now: Millis) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        if let Some(parent_id) = parent_id {
            self.require_folder(parent_id)?;
            if self.ancestry_contains(id, parent_id)? {
                return Err(PlaylistError::FolderCycle {
                    folder: id.to_owned(),
                    under: parent_id.to_owned(),
                }
                .into());
            }
        }
        folder.parent_id = parent_id.map(str::to_owned);
        folder.sort_index = self.next_folder_sort_index()?;
        self.record(folder, false, now)
    }

    /// Soft-deletes a folder and promotes everything inside it one level up — a folder
    /// is filing, not content, and removing one must not destroy playlists.
    pub fn delete_folder(&self, id: &str, now: Millis) -> Result<()> {
        let folder = self
            .read(|conn| Folder::fetch(conn, id))?
            .ok_or_else(|| PlaylistError::NoSuchFolder(id.to_owned()))?;
        if folder.deleted {
            return Ok(());
        }
        for mut child in self.folders_in_parent(Some(id))? {
            child.parent_id = folder.parent_id.clone();
            self.record(child, false, now)?;
        }
        for mut list in self.playlists_in_folder(Some(id))? {
            list.folder_id = folder.parent_id.clone();
            self.record(list, false, now)?;
        }
        self.record(folder, true, now)?;
        Ok(())
    }

    pub fn reorder_folder(
        &self,
        id: &str,
        after: Option<&str>,
        before: Option<&str>,
        now: Millis,
    ) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        if after.is_none() && before.is_none() {
            return Ok(folder);
        }
        if after == Some(id) || before == Some(id) {
            return Err(PlaylistError::BadNeighbours.into());
        }
        let lower = after
            .map(|n| self.require_folder(n).map(|f| f.sort_index))
            .transpose()?;
        let upper = before
            .map(|n| self.require_folder(n).map(|f| f.sort_index))
            .transpose()?;
        check_gap(lower.as_deref(), upper.as_deref())?;
        folder.sort_index = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(folder, false, now)
    }
}

// MARK: - Items

impl Store {
    /// Live entries of a playlist, in order.
    pub fn items(&self, playlist_id: &str) -> Result<Vec<PlaylistItem>> {
        self.read(|conn| {
            PlaylistItem::fetch_where(
                conn,
                "playlistId = ? AND deleted = 0 ORDER BY position, rowid",
                [playlist_id],
            )
        })
    }

    /// Counted in SQLite: a library view shows a count beside every playlist.
    pub fn item_count(&self, playlist_id: &str) -> Result<i64> {
        self.read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM playlistItem WHERE playlistId = ? AND deleted = 0",
                [playlist_id],
                |row| row.get(0),
            )
        })
    }

    /// Appends tracks in the order given. A track already present is added again: a
    /// set list may play the same song twice.
    pub fn add_tracks(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        self.require_playlist(playlist_id)?;
        let last = self.last_position(playlist_id)?;
        self.insert_items(links, playlist_id, last.as_deref(), None, now)
    }

    /// Inserts tracks so the first of them ends up at `index`.
    pub fn insert_tracks(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        index: usize,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        self.require_playlist(playlist_id)?;
        let existing = self.items(playlist_id)?;
        let target = index.min(existing.len());
        let lower = (target > 0).then(|| existing[target - 1].position.clone());
        let upper = existing.get(target).map(|item| item.position.clone());
        self.insert_items(links, playlist_id, lower.as_deref(), upper.as_deref(), now)
    }

    /// Drag-to-reorder. Writes exactly one row. `index` is where the entry ends up,
    /// measured in the list with it already taken out.
    pub fn move_item(
        &self,
        item_id: &str,
        playlist_id: &str,
        index: usize,
        now: Millis,
    ) -> Result<PlaylistItem> {
        let mut item = self.require_item(item_id)?;
        if item.playlist_id != playlist_id {
            return Err(PlaylistError::ItemNotInPlaylist {
                item: item_id.to_owned(),
                playlist: playlist_id.to_owned(),
            }
            .into());
        }
        let others: Vec<PlaylistItem> = self
            .items(playlist_id)?
            .into_iter()
            .filter(|other| other.id != item_id)
            .collect();
        let target = index.min(others.len());
        let lower = (target > 0).then(|| others[target - 1].position.clone());
        let upper = others.get(target).map(|other| other.position.clone());

        // A drag that lands back where it started manufactures no op.
        let already_above = lower.as_ref().is_none_or(|lower| *lower < item.position);
        let already_below = upper.as_ref().is_none_or(|upper| item.position < *upper);
        if already_above && already_below {
            return Ok(item);
        }
        item.position = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(item, false, now)
    }

    /// Soft-deletes an entry; removing it twice mints nothing.
    pub fn remove_item(&self, item_id: &str, now: Millis) -> Result<()> {
        let item = self
            .read(|conn| PlaylistItem::fetch(conn, item_id))?
            .ok_or_else(|| PlaylistError::NoSuchItem(item_id.to_owned()))?;
        if item.deleted {
            return Ok(());
        }
        self.record(item, true, now)?;
        Ok(())
    }
}

// MARK: - Relinking

impl Store {
    /// Repairs entries whose Jellyfin id stopped resolving, by content key.
    ///
    /// `live` answers "does this id still exist", `resolve` "what id does this
    /// fingerprint have now". An entry that matches nothing is *marked*, never deleted —
    /// the file may be on an unmounted drive.
    pub fn relink_items(
        &self,
        live: impl Fn(&str) -> bool,
        resolve: impl Fn(&str) -> Option<String>,
        now: Millis,
    ) -> Result<RelinkReport> {
        let items =
            self.read(|conn| PlaylistItem::fetch_where(conn, "deleted = 0 ORDER BY rowid", []))?;
        let mut report = RelinkReport {
            checked: items.len(),
            ..RelinkReport::default()
        };

        for mut item in items {
            let current = item.linked_jellyfin_id().to_owned();
            if live(&current) {
                if !item.is_unresolved() {
                    continue;
                }
                item.jellyfin_id = current;
                self.record(item, false, now)?;
                report.recovered += 1;
                continue;
            }
            if let Some(replacement) = resolve(&item.content_key) {
                // The resolver handing back the stored id contradicts `live`; trust
                // `live` and write nothing.
                if replacement == current && !item.is_unresolved() {
                    continue;
                }
                item.jellyfin_id = replacement;
                self.record(item, false, now)?;
                report.relinked += 1;
                continue;
            }
            if item.is_unresolved() {
                continue;
            }
            item.jellyfin_id = format!("{UNRESOLVED_PREFIX}{current}");
            self.record(item, false, now)?;
            report.unresolved += 1;
        }
        Ok(report)
    }

    /// Relinks against the local track cache — the pass to run after a library scan.
    /// An empty cache means the scan has not run yet, not that the library vanished.
    pub fn relink_items_against_track_cache(&self, now: Millis) -> Result<RelinkReport> {
        let cache = self.read(TrackRef::fetch_all)?;
        if cache.is_empty() {
            return Ok(RelinkReport::default());
        }
        let live: HashSet<String> = cache
            .iter()
            .map(|track| track.jellyfin_id.clone())
            .collect();
        // Last one wins on a duplicate fingerprint: either id is the same recording.
        let by_content_key: HashMap<String, String> = cache
            .iter()
            .map(|track| (track.content_key.clone(), track.jellyfin_id.clone()))
            .collect();
        self.relink_items(
            |id| live.contains(id),
            |key| by_content_key.get(key).cloned(),
            now,
        )
    }
}

// MARK: - Importing from Jellyfin

impl Store {
    /// The local copy of a Jellyfin playlist, if one has been imported.
    pub fn imported_playlist(&self, source_jellyfin_id: &str) -> Result<Option<Playlist>> {
        Ok(self
            .read(|conn| {
                Playlist::fetch_where(
                    conn,
                    "sourceJellyfinId = ? AND deleted = 0 ORDER BY rowid",
                    [source_jellyfin_id],
                )
            })?
            .into_iter()
            .next())
    }

    /// Which Jellyfin playlists already have a live local copy.
    pub fn imported_source_ids(&self) -> Result<BTreeSet<String>> {
        self.read(|conn| {
            let mut statement =
                conn.prepare("SELECT sourceJellyfinId FROM playlist WHERE sourceJellyfinId IS NOT NULL AND deleted = 0")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        })
    }

    /// Copies a Jellyfin playlist into a local one, or refreshes the copy that exists —
    /// a snapshot, replaced outright on refresh, with every removal an op so the other
    /// devices drop the old contents too. Never automatic: an automatic read-back of
    /// an exported playlist would loop.
    pub fn import_playlist(
        &self,
        name: &str,
        source_jellyfin_id: &str,
        tracks: &[TrackLink],
        now: Millis,
    ) -> Result<Playlist> {
        let playlist = match self.imported_playlist(source_jellyfin_id)? {
            Some(existing) => {
                for item in self.items(&existing.id)? {
                    self.remove_item(&item.id, now)?;
                }
                existing
            }
            None => {
                let mut created = self.create_playlist(name, None, None, now)?;
                created.source_jellyfin_id = Some(source_jellyfin_id.to_owned());
                self.record(created, false, now)?
            }
        };
        self.add_tracks(tracks, &playlist.id, now)?;
        Ok(playlist)
    }
}

// MARK: - Cover art

impl Store {
    /// Sets or clears the item whose cover stands in for a playlist's. `None` when the
    /// playlist is gone.
    pub fn set_artwork(
        &self,
        item_id: Option<&str>,
        playlist_id: &str,
        now: Millis,
    ) -> Result<Option<Playlist>> {
        let Some(mut list) = self.playlist(playlist_id)? else {
            return Ok(None);
        };
        list.artwork_item_id = item_id.map(str::to_owned);
        Ok(Some(self.record(list, false, now)?))
    }

    /// Up to `limit` distinct albums per playlist, in playing order, for drawing a cover
    /// from what it holds — for every playlist in one query.
    pub fn cover_albums(
        &self,
        playlist_ids: &[String],
        limit: usize,
    ) -> Result<HashMap<String, Vec<String>>> {
        if playlist_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.read(|conn| {
            let mut covers: HashMap<String, Vec<String>> = HashMap::new();
            for chunk in playlist_ids.chunks(crate::history::MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT i.playlistId, t.albumId FROM playlistItem i \
                     JOIN track t ON t.jellyfinId = i.jellyfinId \
                     WHERE i.deleted = 0 AND t.albumId IS NOT NULL AND i.playlistId IN ({}) \
                     ORDER BY i.playlistId, i.position",
                    placeholders(chunk.len())
                ))?;
                let rows = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (playlist_id, album_id) = row?;
                    let albums = covers.entry(playlist_id).or_default();
                    if albums.len() < limit && !albums.contains(&album_id) {
                        albums.push(album_id);
                    }
                }
            }
            Ok(covers)
        })
    }
}

// MARK: - Internals

impl Store {
    fn require_playlist(&self, id: &str) -> Result<Playlist> {
        self.playlist(id)?
            .ok_or_else(|| PlaylistError::NoSuchPlaylist(id.to_owned()).into())
    }

    fn require_folder(&self, id: &str) -> Result<Folder> {
        self.folder(id)?
            .ok_or_else(|| PlaylistError::NoSuchFolder(id.to_owned()).into())
    }

    fn require_item(&self, id: &str) -> Result<PlaylistItem> {
        self.read(|conn| PlaylistItem::fetch(conn, id))?
            .filter(|item| !item.deleted)
            .ok_or_else(|| PlaylistError::NoSuchItem(id.to_owned()).into())
    }

    /// Whether `folder_id` lies on the parent chain from `start_id`. Terminates on a
    /// cycle it did not build: two offline moves can merge into one, and a repeat
    /// visit is reported as the cycle it is.
    fn ancestry_contains(&self, folder_id: &str, start_id: &str) -> Result<bool> {
        self.read(|conn| {
            let mut cursor = Some(start_id.to_owned());
            let mut seen = HashSet::new();
            while let Some(id) = cursor {
                if id == folder_id || !seen.insert(id.clone()) {
                    return Ok(true);
                }
                cursor = Folder::fetch(conn, &id)?.and_then(|folder| folder.parent_id);
            }
            Ok(false)
        })
    }

    fn next_playlist_sort_index(&self) -> Result<String> {
        let last = self.read(|conn| {
            last_key(
                conn,
                "SELECT sortIndex FROM playlist WHERE deleted = 0 ORDER BY sortIndex DESC LIMIT 1",
                None,
            )
        })?;
        Ok(FractionalIndex::between(last.as_deref(), None))
    }

    fn next_folder_sort_index(&self) -> Result<String> {
        let last = self.read(|conn| {
            last_key(
                conn,
                "SELECT sortIndex FROM folder WHERE deleted = 0 ORDER BY sortIndex DESC LIMIT 1",
                None,
            )
        })?;
        Ok(FractionalIndex::between(last.as_deref(), None))
    }

    fn last_position(&self, playlist_id: &str) -> Result<Option<String>> {
        self.read(|conn| {
            last_key(
                conn,
                "SELECT position FROM playlistItem WHERE playlistId = ? AND deleted = 0 ORDER BY position DESC LIMIT 1",
                Some(playlist_id),
            )
        })
    }

    fn insert_items(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        lower: Option<&str>,
        upper: Option<&str>,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        let mut created = Vec::with_capacity(links.len());
        for (link, position) in links.iter().zip(positions(links.len(), lower, upper)) {
            created.push(self.record(
                PlaylistItem::new(
                    playlist_id,
                    &link.jellyfin_id,
                    &link.content_key,
                    position,
                    now,
                    self.device_id(),
                ),
                false,
                now,
            )?);
        }
        Ok(created)
    }
}

fn last_key(
    conn: &Connection,
    sql: &str,
    parameter: Option<&str>,
) -> rusqlite::Result<Option<String>> {
    match parameter {
        Some(parameter) => conn
            .query_row(sql, [parameter], |row| row.get(0))
            .optional(),
        None => conn.query_row(sql, [], |row| row.get(0)).optional(),
    }
}

/// Appending counts upwards; inserting into the middle bisects what is left of the gap.
fn positions(count: usize, lower: Option<&str>, upper: Option<&str>) -> Vec<String> {
    let Some(upper) = upper else {
        return FractionalIndex::sequence(lower, count);
    };
    let mut keys = Vec::with_capacity(count);
    let mut previous = lower.map(str::to_owned);
    for _ in 0..count {
        let key = FractionalIndex::between(previous.as_deref(), Some(upper));
        keys.push(key.clone());
        previous = Some(key);
    }
    keys
}

/// The index only asserts on this in debug builds; checking turns a corrupted order in
/// a release build into an error the UI can ignore.
fn check_gap(lower: Option<&str>, upper: Option<&str>) -> Result<()> {
    if let (Some(lower), Some(upper)) = (lower, upper)
        && lower >= upper
    {
        return Err(PlaylistError::BadNeighbours.into());
    }
    Ok(())
}
