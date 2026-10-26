//! Keeping entries linked to their tracks across a Jellyfin rescan.

use std::collections::{HashMap, HashSet};

use crate::Millis;
use crate::error::Result;
use crate::models::{PlaylistItem, TrackRef};
use crate::record::Record;
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
